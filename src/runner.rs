//! EXEC: runner propio sobre `std::process`. Reemplaza a `exec_node_js`, que
//! no tiene timeout, mezcla stdout con stderr en el error y no expone el exit
//! code (delta v1.2 §1 §3.3).
//!
//! Carril A: el programa se escribe a un `.cjs` y se corre con `node archivo`.
//! Node lo envuelve en la funcion de CommonJS: sloppy mode, top-level privado.
//! Carril B: `.mjs`, modulo ES (estricto, `this` top-level `undefined`).
//!
//! A y B corren con el mismo path, el mismo preload y TZ=UTC: cualquier cosa
//! que el programa pueda leer del entorno es identica en los dos lados.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::minify::SourceKind;

/// Determinismo + observador + excepciones. Ver `preload.js`.
pub const PRELOAD: &str = include_str!("preload.js");

/// Exit code con el que el preload termina un programa que lanzo sin atrapar.
/// Es un resultado comparable, no un descarte: el tipo va en stdout.
pub const THROW_EXIT: i32 = 70;

/// Prefijo de las lineas que escribe difuzz, no el programa.
pub const DZ: &str = "!dz ";

/// Linea de stderr donde el preload deja los flags del sensor de asunciones.
pub const FLAGS_TAG: &str = "__DIFUZZ_FLAGS__";

/// Exit code del preload cuando un modulo no enlaza (import que no resuelve,
/// export pedido que no existe). Carril B: el programa no es un caso. No es
/// THROW_EXIT porque no es un throw comparable.
pub const LINK_EXIT: i32 = 71;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Exited(i32),
    /// Termino por senal (sin exit code). En Linux: crash, OOM killer, etc.
    Signal,
    /// Supero el timeout y fue matado.
    Timeout,
    /// No se pudo escribir el archivo o lanzar `node`. Error del fuzzer, no del
    /// programa: main lo trata aparte.
    Failed(String),
}

pub struct Exec {
    pub status: Status,
    pub stdout: String,
    pub stderr: String,
    /// Flags del sensor de asunciones (preload). Vacio si el proceso no llego
    /// a salir solo (timeout, senal).
    pub flags: Vec<String>,
    pub elapsed: Duration,
}

impl Exec {
    /// Termino por si mismo: exit 0 o lanzando (THROW_EXIT).
    pub fn completed(&self) -> bool {
        matches!(self.status, Status::Exited(0) | Status::Exited(THROW_EXIT))
    }

    pub fn threw(&self) -> bool {
        self.status == Status::Exited(THROW_EXIT)
    }

    /// Algo que comparar: el programa imprimio, o lanzo (y el tipo esta en stdout).
    pub fn ok_with_output(&self) -> bool {
        self.completed() && (self.threw() || self.own_output())
    }

    pub fn ok_silent(&self) -> bool {
        self.completed() && !self.threw() && !self.own_output()
    }

    /// El preload vio globals creados por el programa (`!dz global …`).
    pub fn has_globals(&self) -> bool {
        self.stdout.lines().any(|l| l.starts_with("!dz global "))
    }

    /// Lineas escritas por el programa, no por difuzz.
    fn own_output(&self) -> bool {
        self.stdout
            .lines()
            .any(|l| !l.trim().is_empty() && !l.starts_with(DZ))
    }

    fn failed(msg: String, start: Instant) -> Exec {
        Exec {
            status: Status::Failed(msg),
            stdout: String::new(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: start.elapsed(),
        }
    }
}

/// El preload escrito a disco una vez por proceso; lo comparten todos los
/// runners y se borra al soltar el guard.
pub struct Preload {
    path: PathBuf,
}

impl Preload {
    pub fn write() -> std::io::Result<Preload> {
        let path = std::env::temp_dir().join(format!("difuzz-{}-preload.cjs", std::process::id()));
        std::fs::write(&path, PRELOAD)?;
        Ok(Preload { path })
    }
}

impl Drop for Preload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct Runner {
    /// Un solo path por worker: A y B de un mismo caso corren en secuencia en
    /// el mismo worker, y un programa que imprime su propio filename
    /// (`module`, stacks) tiene que ver lo mismo en los dos lados.
    script: PathBuf,
    preload: PathBuf,
    timeout: Duration,
    /// Carril B: `.mjs` en vez de `.cjs` (ver `with_kind`).
    kind: SourceKind,
    /// Carril B, pasada observada: el envoltorio que importa `script` y
    /// serializa su namespace. Al lado de `script`, en el mismo directorio.
    wrapper: PathBuf,
    /// Corre `wrapper` en vez de `script` (ver `observing`).
    observe: bool,
}

impl Runner {
    pub fn new(preload: &Preload, worker: usize, timeout: Duration) -> Runner {
        Runner {
            script: std::env::temp_dir()
                .join(format!("difuzz-{}-w{worker}.cjs", std::process::id())),
            preload: preload.path.clone(),
            timeout,
            kind: SourceKind::Script,
            wrapper: std::env::temp_dir()
                .join(format!("difuzz-{}-w{worker}-observar.mjs", std::process::id())),
            observe: false,
        }
    }

    /// Carril B: el programa se escribe a un `.mjs` y node lo corre como modulo
    /// ES. `--require` con el preload `.cjs` sigue andando (verificado con node
    /// 24): el preload corre antes de cargar el modulo.
    pub fn with_kind(mut self, kind: SourceKind) -> Runner {
        self.script.set_extension(kind.ext());
        self.kind = kind;
        self
    }

    /// Un runner que, en vez del programa, ejecuta el envoltorio observador de
    /// modulos: mismo `script` (A y B siguen viendo el mismo path), mismo
    /// preload. Lo que se compara es la salida del programa mas su namespace de
    /// exports. No borra nada al soltarse: los archivos son del runner original.
    pub fn observing(&self) -> Runner {
        Runner {
            script: self.script.clone(),
            preload: self.preload.clone(),
            timeout: self.timeout,
            kind: self.kind,
            wrapper: self.wrapper.clone(),
            observe: true,
        }
    }

    pub fn run(&self, js: &str) -> Exec {
        self.run_inner(js, false)
    }

    /// Igual, pero el preload cambia el contenido de `Function.prototype.toString()`
    /// y de `.stack`. Si la salida no cambia, el programa no depende de ellos.
    pub fn run_salted(&self, js: &str) -> Exec {
        self.run_inner(js, true)
    }

    /// `node --check`: si node no acepta el fuente, no es un caso. El parser de SWC
    /// es mas permisivo (p. ej. claves duplicadas en un patron de parametros).
    pub fn node_accepts(&self, js: &str) -> bool {
        if std::fs::write(&self.script, js).is_err() {
            return true;
        }
        Command::new("node")
            .arg("--check")
            .arg(&self.script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_or(true, |s| s.success())
    }

    fn run_inner(&self, js: &str, salted: bool) -> Exec {
        let start = Instant::now();
        if let Err(e) = std::fs::write(&self.script, js) {
            return Exec::failed(format!("escribir {}: {e}", self.script.display()), start);
        }
        if self.observe {
            let name = self.script.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Err(e) = std::fs::write(&self.wrapper, module_wrapper(&name)) {
                return Exec::failed(format!("escribir {}: {e}", self.wrapper.display()), start);
            }
            return run_file(&self.preload, &self.wrapper, self.timeout, start, salted);
        }
        run_file(&self.preload, &self.script, self.timeout, start, salted)
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if self.observe {
            return;
        }
        let _ = std::fs::remove_file(&self.script);
        let _ = std::fs::remove_file(&self.wrapper);
    }
}

/// Envoltorio observador de modulos (carril B). Importa el programa (la misma
/// instancia que evalua node: un solo modulo por URL) y, cuando termino de
/// evaluarse (top-level await incluido), le pasa el namespace al preload.
///
/// Sin efecto observador: el fuente que se minifica no cambia, y los exports ya
/// son usados por ser exports. Si el programa lanza, el envoltorio no llega a
/// correr y el throw sale como siempre (onThrow del preload).
pub fn module_wrapper(program_file: &str) -> String {
    // El nombre lo arma el runner (difuzz-<pid>-w<n>.mjs), pero igual va escapado.
    let spec = program_file.replace('\\', "\\\\").replace('"', "\\\"");
    format!("import * as ns from \"./{spec}\";\nglobalThis.__difuzzObserveExports(ns);\n")
}

fn run_file(preload: &Path, script: &Path, timeout: Duration, start: Instant, salted: bool) -> Exec {
    let mut cmd = Command::new("node");
    // --no-warnings: los avisos de node (p. ej. "Invalid asm.js") traen el PID y
    // no son salida del programa.
    cmd.arg("--no-warnings")
        .arg("--require")
        .arg(preload)
        .arg(script)
        .env("TZ", "UTC");
    if salted {
        cmd.env("DIFUZZ_SALT", "1");
    }
    let mut child = match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Exec::failed(format!("spawn node: {e}"), start),
    };

    // Los pipes se drenan en hilos propios. Si no, un programa que imprime mas
    // que el buffer del pipe se bloquea escribiendo y nosotros esperando: deadlock
    // que el timeout reportaria como "cuelgue", falso.
    let mut out_pipe = child.stdout.take().expect("stdout fue pedido como piped");
    let mut err_pipe = child.stderr.take().expect("stderr fue pedido como piped");
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });

    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                break match st.code() {
                    Some(code) => Status::Exited(code),
                    None => Status::Signal,
                };
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Status::Timeout;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Status::Failed(format!("wait: {e}"));
            }
        }
    };

    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();

    let (stderr, flags) = split_flags(String::from_utf8_lossy(&stderr).into_owned());
    Exec {
        status,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr,
        flags,
        elapsed: start.elapsed(),
    }
}

/// Saca la linea de flags del stderr y la parsea.
fn split_flags(stderr: String) -> (String, Vec<String>) {
    let Some(i) = stderr.rfind(FLAGS_TAG) else {
        return (stderr, Vec::new());
    };
    let end = stderr[i..].find('\n').map_or(stderr.len(), |j| i + j);
    let flags = stderr[i + FLAGS_TAG.len()..end]
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(String::from)
        .collect();
    let mut rest = stderr[..i].trim_end_matches('\n').to_string();
    rest.push_str(&stderr[end..]);
    (rest, flags)
}

#[cfg(test)]
mod tests {
    use super::split_flags;

    #[test]
    fn envoltorio_de_modulos() {
        assert_eq!(
            super::module_wrapper("difuzz-7-w0.mjs"),
            "import * as ns from \"./difuzz-7-w0.mjs\";\nglobalThis.__difuzzObserveExports(ns);\n"
        );
        assert!(super::module_wrapper("a\"b.mjs").contains("\"./a\\\"b.mjs\""));
    }

    /// Node de verdad sobre un `.mjs` con el preload `.cjs` por `--require`. Si
    /// no hay `node` en PATH, no prueba nada (y lo dice).
    #[test]
    fn modulo_con_preload_en_node() {
        use super::{Preload, Runner, Status, LINK_EXIT};
        use crate::minify::SourceKind;
        use std::time::Duration;

        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let runner = Runner::new(&preload, 9999, Duration::from_secs(20)).with_kind(SourceKind::Module);

        // Top-level await, `this` de modulo, un flag del sensor y un export en TDZ.
        let js = "export const a = [1, 2];\nexport let t;\nawait new Promise((r) => setTimeout(r, 10));\nt = typeof this;\nnew Proxy({}, {});\n";
        let crudo = runner.run(js);
        assert_eq!(crudo.status, Status::Exited(0), "stderr: {}", crudo.stderr);
        assert!(crudo.ok_silent(), "stdout: {}", crudo.stdout);
        assert_eq!(crudo.flags, ["proxy"]);

        let obs = runner.observing().run(js);
        assert_eq!(obs.status, Status::Exited(0), "stderr: {}", obs.stderr);
        assert_eq!(obs.stdout, "export a = [1, 2]\nexport t = \"undefined\"\n");
        assert_eq!(obs.flags, ["proxy"]);

        // El namespace sale en orden de nombre, no de declaracion.
        let orden = runner.observing().run("export { x };\nlet y = 1;\nexport { y as b };\nlet x = 2;\n");
        assert_eq!(orden.stdout, "export b = 1\nexport x = 2\n");

        // Un import que no resuelve no es un throw comparable: es LINK_EXIT.
        let link = runner.run("import { z } from './no-existe-difuzz.mjs';\nconsole.log(z);\n");
        assert_eq!(link.status, Status::Exited(LINK_EXIT), "stdout: {}", link.stdout);
        assert!(!link.completed());
        // Un throw comun sigue siendo THROW_EXIT, con flags.
        let thr = runner.run("throw new TypeError('x');\n");
        assert!(thr.threw());
        assert_eq!(thr.stdout, "!dz throw TypeError\n");
    }

    #[test]
    fn separa_los_flags_del_stderr() {
        let (rest, flags) = split_flags("warn\n\n__DIFUZZ_FLAGS__ proxy,stack\n".into());
        assert_eq!(rest, "warn\n");
        assert_eq!(flags, ["proxy", "stack"]);
        let (rest, flags) = split_flags("__DIFUZZ_FLAGS__ \n".into());
        assert_eq!(rest, "\n");
        assert!(flags.is_empty());
        let (rest, flags) = split_flags("nada".into());
        assert_eq!(rest, "nada");
        assert!(flags.is_empty());
    }
}
