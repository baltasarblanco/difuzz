//! EXEC: runner propio sobre `std::process`. Reemplaza a `exec_node_js`, que
//! no tiene timeout, mezcla stdout con stderr en el error y no expone el exit
//! code (delta v1.2 §1 §3.3).
//!
//! Carril A: el programa se escribe a un `.cjs` y se corre con `node archivo`.
//! Node lo envuelve en la funcion de CommonJS: sloppy mode, top-level privado.
//!
//! A y B corren con el mismo path, el mismo preload y TZ=UTC: cualquier cosa
//! que el programa pueda leer del entorno es identica en los dos lados.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Determinismo + observador + excepciones. Ver `preload.js`.
pub const PRELOAD: &str = include_str!("preload.js");

/// Exit code con el que el preload termina un programa que lanzo sin atrapar.
/// Es un resultado comparable, no un descarte: el tipo va en stdout.
pub const THROW_EXIT: i32 = 70;

/// Prefijo de las lineas que escribe difuzz, no el programa.
pub const DZ: &str = "!dz ";

/// Linea de stderr donde el preload deja los flags del sensor de asunciones.
pub const FLAGS_TAG: &str = "__DIFUZZ_FLAGS__";

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
}

impl Runner {
    pub fn new(preload: &Preload, worker: usize, timeout: Duration) -> Runner {
        Runner {
            script: std::env::temp_dir()
                .join(format!("difuzz-{}-w{worker}.cjs", std::process::id())),
            preload: preload.path.clone(),
            timeout,
        }
    }

    pub fn run(&self, js: &str) -> Exec {
        let start = Instant::now();
        if let Err(e) = std::fs::write(&self.script, js) {
            return Exec::failed(format!("escribir {}: {e}", self.script.display()), start);
        }
        run_file(&self.preload, &self.script, self.timeout, start)
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.script);
    }
}

fn run_file(preload: &Path, script: &Path, timeout: Duration, start: Instant) -> Exec {
    let mut child = match Command::new("node")
        .arg("--require")
        .arg(preload)
        .arg(script)
        .env("TZ", "UTC")
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
