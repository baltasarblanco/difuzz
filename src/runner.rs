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
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::minify::SourceKind;

/// Determinismo + observador + excepciones. Ver `preload.js`.
pub const PRELOAD: &str = include_str!("preload.js");

/// Exit code con el que el preload termina un programa que lanzo sin atrapar.
/// Es un resultado comparable, no un descarte: el tipo va en stdout.
pub const THROW_EXIT: i32 = 70;

/// Prefijo de las lineas que escribe difuzz, no el programa.
pub const DZ: &str = "!dz ";


/// M16: el doble del limite de pila por defecto de V8 (984 KB: `node --v8-options`, v24.14.1), para
/// distinguir un RangeError por profundidad del motor de uno del programa.
pub const DEEP_STACK: &str = "--stack-size=1968";

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
    /// M10: lo que el preload escribio por su canal de control (DIFUZZ_CTL):
    /// `throw <Tipo>`, `link <code>`, `flags a,b`, `dz`, `global`. El programa no
    /// lo ve ni lo puede escribir (salvo adivinando el fd).
    pub ctl: String,
}

impl Exec {
    /// Termino por si mismo: exit 0 o lanzando (THROW_EXIT).
    pub fn completed(&self) -> bool {
        self.status == Status::Exited(0) || self.threw()
    }

    /// M10: lanzo sin atrapar segun el preload (exit 70 **y** `throw` en el
    /// canal). Un programa que sale con 70 por su cuenta no lanzo.
    pub fn threw(&self) -> bool {
        self.status == Status::Exited(THROW_EXIT) && self.ctl_count("throw ") > 0
    }

    /// M10: no enlazo (exit 71 **y** `link` en el canal).
    pub fn link_failed(&self) -> bool {
        self.status == Status::Exited(LINK_EXIT) && self.ctl_count("link ") > 0
    }

    /// M21: mensaje y stack del throw no atrapado (linea `exc` del canal, con
    /// `\n` y `\\` escapados), para guardar junto al hallazgo. No se compara.
    pub fn exc(&self) -> Option<String> {
        let esc = self.ctl.lines().rev().find_map(|l| l.strip_prefix("exc "))?;
        let mut out = String::with_capacity(esc.len());
        let mut it = esc.chars();
        while let Some(c) = it.next() {
            match (c, c == '\\') {
                (_, true) => match it.next() {
                    Some('n') => out.push('\n'),
                    Some(o) => out.push(o),
                    None => out.push('\\'),
                },
                (c, false) => out.push(c),
            }
        }
        Some(out)
    }

    /// Lineas del canal que empiezan con `prefix` (o que son exactamente
    /// `prefix` sin el espacio final: `dz`, `global`).
    fn ctl_count(&self, prefix: &str) -> usize {
        self.ctl
            .lines()
            .filter(|l| l.starts_with(prefix) || *l == prefix.trim_end())
            .count()
    }

    /// Algo que comparar: el programa imprimio, o lanzo (y el tipo esta en stdout).
    pub fn ok_with_output(&self) -> bool {
        self.completed() && (self.threw() || self.own_output())
    }

    pub fn ok_silent(&self) -> bool {
        self.completed() && !self.threw() && !self.own_output()
    }

    /// El preload vio globals creados por el programa (M10: contados en el
    /// canal, no en stdout, donde el programa puede imprimir `!dz global`).
    pub fn has_globals(&self) -> bool {
        self.ctl_count("global") > 0
    }

    /// Lineas escritas por el programa, no por difuzz. M10: las lineas no vacias
    /// de stdout son mas que las `!dz` que el preload declaro en el canal (antes,
    /// una linea `!dz ` del programa no contaba como salida propia).
    fn own_output(&self) -> bool {
        let lines = self.stdout.lines().filter(|l| !l.trim().is_empty()).count();
        lines > self.ctl_count("dz")
    }

    fn failed(msg: String, start: Instant) -> Exec {
        Exec {
            status: Status::Failed(msg),
            stdout: String::new(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: start.elapsed(),
            ctl: String::new(),
        }
    }
}

/// El preload escrito a disco una vez por proceso; lo comparten todos los
/// runners y se borra al soltar el guard.
pub struct Preload {
    path: PathBuf,
}

/// M23: todos los temporales de una corrida en `<tmp>/difuzz-<pid>-tmp/`. El
/// prefijo `difuzz-<pid>-` lo siguen reconociendo normalize_paths (oracle.rs) y
/// la cache (cache.rs); `clean_stale` borra los de procesos muertos.
pub fn run_dir() -> PathBuf {
    std::env::temp_dir().join(format!("difuzz-{}-tmp", std::process::id()))
}

/// M23: borra de `<tmp>` lo que dejaron corridas cortadas (abort, SIGKILL): las
/// entradas `difuzz-<pid>-...` cuyo pid ya no existe (`/proc/<pid>`). Devuelve
/// cuantas borro.
pub fn clean_stale() -> usize {
    clean_stale_in(&std::env::temp_dir())
}

fn clean_stale_in(tmp: &Path) -> usize {
    let own = std::process::id().to_string();
    let Ok(dir) = std::fs::read_dir(tmp) else {
        return 0;
    };
    let mut n = 0;
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("difuzz-") else {
            continue;
        };
        let pid: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if pid.is_empty() || pid == own || !rest[pid.len()..].starts_with('-') {
            continue;
        }
        if Path::new("/proc").join(&pid).exists() {
            continue;
        }
        let ok = match e.file_type() {
            Ok(t) if t.is_dir() => std::fs::remove_dir_all(e.path()).is_ok(),
            Ok(_) => std::fs::remove_file(e.path()).is_ok(),
            Err(_) => false,
        };
        n += usize::from(ok);
    }
    n
}

impl Preload {
    pub fn write() -> std::io::Result<Preload> {
        std::fs::create_dir_all(run_dir())?;
        let path = run_dir().join(format!("difuzz-{}-preload.cjs", std::process::id()));
        std::fs::write(&path, PRELOAD)?;
        Ok(Preload { path })
    }

    /// Para tests en paralelo: un preload con nombre propio (el de `write` es
    /// uno por proceso y el primero que se suelta lo borra).
    #[cfg(test)]
    pub fn write_tagged(tag: &str) -> std::io::Result<Preload> {
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-{tag}.cjs", std::process::id()));
        std::fs::write(&path, PRELOAD)?;
        Ok(Preload { path })
    }
}

impl Drop for Preload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        // Vacio si los runners ya se soltaron (main los suelta antes).
        if self.path.parent() == Some(run_dir().as_path()) {
            let _ = std::fs::remove_dir(run_dir());
        }
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
    /// M12: con `--invocar`, el envoltorio de modulos ademas llama a las
    /// funciones exportadas (`__difuzzObserveExportsInvocar`).
    invocar: bool,
    /// M10: canal de control del preload, uno por worker, truncado antes de
    /// cada ejecucion.
    ctl: PathBuf,
    /// M11: cwd del programa, uno por worker y la MISMA ruta para A y B; se
    /// vacia antes de cada ejecucion (sin estado compartido entre A y B).
    cwd: PathBuf,
    /// M30 (`--stubs`): manifiesto de stubs del caso (stubs.rs), uno por
    /// worker. `None`: sin stubs (no se pasa DIFUZZ_STUBS).
    stubs: Option<PathBuf>,
}

impl Runner {
    pub fn new(preload: &Preload, worker: usize, timeout: Duration) -> Runner {
        let dir = run_dir();
        // Si falla, el primer `run` lo reporta como Failed (no se puede escribir).
        let _ = std::fs::create_dir_all(&dir);
        let pid = std::process::id();
        Runner {
            script: dir.join(format!("difuzz-{pid}-w{worker}.cjs")),
            preload: preload.path.clone(),
            timeout,
            kind: SourceKind::Script,
            wrapper: dir.join(format!("difuzz-{pid}-w{worker}-observar.mjs")),
            observe: false,
            invocar: false,
            ctl: dir.join(format!("difuzz-{pid}-w{worker}-ctl.txt")),
            cwd: dir.join(format!("difuzz-{pid}-w{worker}-cwd")),
            stubs: None,
        }
    }

    /// M30: con `--stubs`, los specifiers que no resuelven pasan a stubs (ver
    /// stubs.rs y el final de preload.js). El manifiesto lo fija `set_stubs`.
    pub fn with_stubs(mut self, on: bool) -> Runner {
        self.stubs = on.then(|| self.ctl.with_file_name(
            self.ctl.file_name().map_or_else(String::new, |n| n.to_string_lossy().replace("-ctl.txt", "-stubs.json")),
        ));
        self
    }

    /// M30: el manifiesto del caso actual (lo calcula main del original). A y B
    /// corren con el mismo. Sin `--stubs` no hace nada.
    pub fn set_stubs(&self, manifest: &str) {
        if let Some(p) = &self.stubs {
            let _ = std::fs::write(p, manifest);
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

    /// M12: `--invocar` tambien en el envoltorio de modulos (carril B).
    pub fn with_invocar(mut self, invocar: bool) -> Runner {
        self.invocar = invocar;
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
            invocar: self.invocar,
            ctl: self.ctl.clone(),
            cwd: self.cwd.clone(),
            stubs: self.stubs.clone(),
        }
    }

    pub fn run(&self, js: &str) -> Exec {
        self.run_inner(js, Opts::default())
    }

    /// M28: lo que distingue la ejecucion de `run` en la cache de A ademas del
    /// texto: el envoltorio observador (y si invoca) corre otro archivo.
    pub fn cache_tag(&self) -> &'static str {
        match (self.observe, self.invocar) {
            (false, _) => "",
            (true, false) => "observar",
            (true, true) => "observar+invocar",
        }
    }

    /// Igual, pero el preload cambia el contenido de `Function.prototype.toString()`
    /// y de `.stack`. Si la salida no cambia, el programa no depende de ellos.
    pub fn run_salted(&self, js: &str) -> Exec {
        self.run_inner(js, Opts { salted: true, ..Opts::default() })
    }

    /// M3: igual que `run` con otro timeout (reintento de un Timeout de B).
    pub fn run_with_timeout(&self, js: &str, timeout: Duration) -> Exec {
        self.run_inner(js, Opts { timeout: Some(timeout), ..Opts::default() })
    }

    /// M16: igual que `run` con argumentos extra para node (p. ej. `--stack-size`).
    pub fn run_with_args(&self, js: &str, node_args: &[&str]) -> Exec {
        self.run_inner(js, Opts { node_args, ..Opts::default() })
    }

    /// Re-chequeo de `global_conocido` (main.rs): igual que `run`, con esos globals
    /// definidos como stubs inertes por el preload (DIFUZZ_GLOBALS).
    pub fn run_with_globals(&self, js: &str, globals: &str) -> Exec {
        self.run_inner(js, Opts { globals: Some(globals), ..Opts::default() })
    }

    /// El timeout de este runner (`--timeout-ms`).
    pub fn timeout(&self) -> Duration {
        self.timeout
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

    fn run_inner(&self, js: &str, opts: Opts) -> Exec {
        let start = Instant::now();
        if let Err(e) = std::fs::write(&self.script, js) {
            return Exec::failed(format!("escribir {}: {e}", self.script.display()), start);
        }
        if self.observe {
            let name = self.script.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Err(e) = std::fs::write(&self.wrapper, module_wrapper(&name, self.invocar)) {
                return Exec::failed(format!("escribir {}: {e}", self.wrapper.display()), start);
            }
            return run_file(self, &self.wrapper, start, opts.timeout.unwrap_or(self.timeout), opts);
        }
        run_file(self, &self.script, start, opts.timeout.unwrap_or(self.timeout), opts)
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        if self.observe {
            return;
        }
        let _ = std::fs::remove_file(&self.script);
        let _ = std::fs::remove_file(&self.wrapper);
        let _ = std::fs::remove_file(&self.ctl);
        let _ = std::fs::remove_dir_all(&self.cwd);
        if let Some(p) = &self.stubs {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Envoltorio observador de modulos (carril B). Importa el programa (la misma
/// instancia que evalua node: un solo modulo por URL) y, cuando termino de
/// evaluarse (top-level await incluido), le pasa el namespace al preload.
///
/// Sin efecto observador: el fuente que se minifica no cambia, y los exports ya
/// son usados por ser exports. Si el programa lanza, el envoltorio no llega a
/// correr y el throw sale como siempre (onThrow del preload).
///
/// `invocar` (M12): llama a `__difuzzObserveExportsInvocar`, que imprime lo mismo
/// y despues llama a cada funcion exportada (preload.js).
pub fn module_wrapper(program_file: &str, invocar: bool) -> String {
    // El nombre lo arma el runner (difuzz-<pid>-w<n>.mjs), pero igual va escapado.
    let spec = program_file.replace('\\', "\\\\").replace('"', "\\\"");
    let observe = if invocar { "__difuzzObserveExportsInvocar" } else { "__difuzzObserveExports" };
    format!("import * as ns from \"./{spec}\";\nglobalThis.{observe}(ns);\n")
}

/// Variantes de una ejecucion: salada, otro timeout, argumentos extra de node.
#[derive(Default, Clone, Copy)]
struct Opts<'a> {
    salted: bool,
    timeout: Option<Duration>,
    node_args: &'a [&'a str],
    /// Nombres separados por coma para DIFUZZ_GLOBALS.
    globals: Option<&'a str>,
}

fn run_file(r: &Runner, script: &Path, start: Instant, timeout: Duration, opts: Opts) -> Exec {
    let (preload, salted) = (&r.preload, opts.salted);
    if let Err(e) = std::fs::write(&r.ctl, "") {
        return Exec::failed(format!("escribir {}: {e}", r.ctl.display()), start);
    }
    // M11: directorio de trabajo vacio, el mismo para A y B. TMPDIR y HOME
    // adentro; del entorno de difuzz solo pasa PATH (y TZ fija).
    let _ = std::fs::remove_dir_all(&r.cwd);
    let tmpdir = r.cwd.join("tmp");
    if let Err(e) = std::fs::create_dir_all(&tmpdir) {
        return Exec::failed(format!("crear {}: {e}", tmpdir.display()), start);
    }
    let mut cmd = Command::new(node_path());
    cmd.current_dir(&r.cwd).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("TMPDIR", &tmpdir).env("HOME", &r.cwd);
    // --no-warnings: los avisos de node (p. ej. "Invalid asm.js") traen el PID y
    // no son salida del programa.
    cmd.arg("--no-warnings")
        .args(opts.node_args)
        .arg("--require")
        .arg(preload)
        .arg(script)
        .env("TZ", "UTC");
    if salted {
        cmd.env("DIFUZZ_SALT", "1");
    }
    cmd.env("DIFUZZ_CTL", &r.ctl);
    if let Some(p) = &r.stubs {
        cmd.env("DIFUZZ_STUBS", p);
    }
    if let Some(g) = opts.globals {
        cmd.env("DIFUZZ_GLOBALS", g);
    }
    // M2: grupo de procesos propio (pgid = pid de node). Al terminar, por timeout
    // o normalmente, se mata el grupo entero: un hijo que hereda el stdout ya no
    // mantiene el pipe abierto ni queda huerfano.
    cmd.process_group(0);
    // R1-H19: mientras este node corre, la cosecha de otro worker no toca a sus huerfanos.
    let en_curso = crate::huerfanos::en_curso(&r.cwd);
    let mut child = match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Exec::failed(format!("spawn node: {e}"), start),
    };
    let pid = child.id();

    // Los pipes se drenan en hilos propios. Si no, un programa que imprime mas
    // que el buffer del pipe se bloquea escribiendo y nosotros esperando: deadlock
    // que el timeout reportaria como "cuelgue", falso. Lo leido se acumula en un
    // buffer compartido: si un proceso escapo del grupo (setsid) y retiene el
    // pipe, el join tiene limite y se usa lo que haya.
    let (tx, rx) = mpsc::channel::<()>();
    let out_buf = drain(child.stdout.take().expect("stdout fue pedido como piped"), tx.clone());
    let err_buf = drain(child.stderr.take().expect("stderr fue pedido como piped"), tx);

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
                    kill_group(pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    break Status::Timeout;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                kill_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                break Status::Failed(format!("wait: {e}"));
            }
        }
    };
    drop(en_curso);
    // Tambien al terminar normalmente: lo que el programa dejo corriendo muere.
    // Riesgo aceptado: node ya fue cosechado y su pid podria reusarse, pero un
    // proceso nuevo solo tiene pgid == pid si se hace lider de grupo.
    kill_group(pid);
    // Subreaper (huerfanos.rs): los `detached` que escaparon del grupo ya son
    // hijos de difuzz; se matan aca. Si los pipes no cierran enseguida, una
    // segunda pasada (un nieto que quedo huerfano al morir su padre por el kill).
    crate::huerfanos::cosechar();
    let mut second_pass = crate::huerfanos::activo();

    let deadline = Instant::now() + READ_GRACE;
    let mut closed = 0;
    while closed < 2 {
        let left = deadline.saturating_duration_since(Instant::now());
        let slice = if second_pass { left.min(Duration::from_millis(50)) } else { left };
        match rx.recv_timeout(slice) {
            Ok(()) => closed += 1,
            Err(_) if second_pass => {
                second_pass = false;
                crate::huerfanos::cosechar();
            }
            Err(_) => break,
        }
    }
    if closed < 2 {
        eprintln!(
            "runner: un proceso escapo del grupo (setsid/detached?) y retiene la salida de {}; se usa lo leido hasta ahora",
            script.display()
        );
    }
    let take = |b: &Arc<Mutex<Vec<u8>>>| std::mem::take(&mut *b.lock().unwrap_or_else(|e| e.into_inner()));
    let stdout = take(&out_buf);
    let stderr = take(&err_buf);

    let ctl = std::fs::read(&r.ctl).map(|b| decode_lossless(&b)).unwrap_or_default();
    Exec {
        status,
        stdout: decode_lossless(&stdout),
        stderr: decode_lossless(&stderr),
        flags: ctl_flags(&ctl),
        elapsed: start.elapsed(),
        ctl,
    }
}

/// M8: UTF-8 sin perdida. El UTF-8 valido queda igual; cada byte invalido pasa
/// a `\u{FFFD}` seguido de su valor en hex (`\u{FFFD}ff`). `from_utf8_lossy`
/// colapsaba bytes distintos en el mismo U+FFFD y dos salidas distintas
/// comparaban iguales. Colision aceptada: un programa que imprima a proposito
/// U+FFFD seguido de dos hex se confunde con el escape.
pub fn decode_lossless(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        for b in chunk.invalid() {
            out.push('\u{FFFD}');
            out.push_str(&format!("{b:02x}"));
        }
    }
    out
}

/// Ruta absoluta de `node`, buscada una vez en el PATH de difuzz. Con
/// `env_clear` + `PATH` (M11), `std` no puede usar posix_spawnp para buscar un
/// programa por nombre y cae a fork+exec: en un proceso con mucha memoria eso
/// duplicaba el tiempo de sistema (MEDIDO con strace: sin CLONE_VFORK). Con la
/// ruta absoluta vuelve a posix_spawn (vfork). Si no la encuentra, "node" (el
/// spawn fallara igual que antes y se reporta como Failed).
fn node_path() -> &'static Path {
    static NODE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    NODE.get_or_init(|| {
        std::env::var_os("PATH")
            .and_then(|p| std::env::split_paths(&p).map(|d| d.join("node")).find(|n| n.is_file()))
            .unwrap_or_else(|| PathBuf::from("node"))
    })
}

/// M2: cuanto se espera el EOF de los pipes despues de que node termino y se
/// mato su grupo. Solo se agota si un proceso escapo del grupo.
const READ_GRACE: Duration = Duration::from_secs(1);

/// Lee `pipe` hasta EOF en un hilo propio, acumulando en un buffer compartido.
/// Avisa por `done` al llegar al EOF (o a un error de lectura).
fn drain(mut pipe: impl Read + Send + 'static, done: mpsc::Sender<()>) -> Arc<Mutex<Vec<u8>>> {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let b = Arc::clone(&buf);
    std::thread::spawn(move || {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => b.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk[..n]),
            }
        }
        let _ = done.send(());
    });
    buf
}

/// M2: SIGKILL al grupo de procesos `pid` (creado con `process_group(0)`).
fn kill_group(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: kill(2) no toca memoria de este proceso. El pid negativo apunta al
    // grupo cuyo lider es node; ESRCH (grupo vacio) se ignora.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

/// M10: los flags del sensor salen del canal de control (la ultima linea
/// `flags`), no del stderr, donde el programa podia escribir uno falso (C3).
fn ctl_flags(ctl: &str) -> Vec<String> {
    ctl.lines()
        .rev()
        .find_map(|l| l.strip_prefix("flags "))
        .map(|f| f.split(',').map(str::trim).filter(|f| !f.is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::ctl_flags;

    #[test]
    fn envoltorio_de_modulos() {
        assert_eq!(
            super::module_wrapper("difuzz-7-w0.mjs", false),
            "import * as ns from \"./difuzz-7-w0.mjs\";\nglobalThis.__difuzzObserveExports(ns);\n"
        );
        assert!(super::module_wrapper("a\"b.mjs", false).contains("\"./a\\\"b.mjs\""));
        assert!(super::module_wrapper("m.mjs", true).ends_with("globalThis.__difuzzObserveExportsInvocar(ns);\n"));
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

    /// M2: un hijo que hereda el stdout no estira el timeout ni queda huerfano.
    #[test]
    fn timeout_mata_el_grupo_y_no_espera_al_hijo() {
        use super::{Preload, Runner, Status};
        use std::time::{Duration, Instant};

        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m2.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let runner = Runner::new(&preload, 9998, Duration::from_millis(500));
        let t0 = Instant::now();
        let e = runner.run("require('child_process').spawn('sleep',['5'],{stdio:'inherit'});\nwhile(true){}\n");
        assert_eq!(e.status, Status::Timeout);
        assert!(t0.elapsed() < Duration::from_millis(1500), "tardo {:?}", t0.elapsed());
    }

    /// M9: en el carril A, un require no atrapado de algo que no existe es
    /// enlace (LINK_EXIT), no un throw comparable. Atrapado, sigue siendo programa.
    #[test]
    fn require_inexistente_es_enlace() {
        use super::{Preload, Runner, Status, LINK_EXIT};
        use std::time::Duration;
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m9.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let runner = Runner::new(&preload, 9997, Duration::from_secs(20));
        let e = runner.run("console.log(1);\nrequire('./no-existe-difuzz');\n");
        assert_eq!(e.status, Status::Exited(LINK_EXIT), "stdout: {}", e.stdout);
        let ok = runner.run("try { require('./no-existe-difuzz'); } catch (e) { console.log(e.code); }\n");
        assert_eq!((ok.status, ok.stdout.as_str()), (Status::Exited(0), "MODULE_NOT_FOUND\n"));
    }

    /// M10: el programa no puede hacerse pasar por el preload (exit 70/71,
    /// lineas `!dz`, flags en stderr) ni evitar que un throw termine.
    #[test]
    fn canal_de_control_no_se_falsifica() {
        use super::{Preload, Runner, Status};
        use std::time::Duration;
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m10.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let r = Runner::new(&preload, 9996, Duration::from_secs(20));
        // exit 70 propio: no lanzo, no termino "solo".
        let e = r.run("console.log(1);\nprocess.exit(70);\n");
        assert_eq!(e.status, Status::Exited(70));
        assert!(!e.threw() && !e.completed());
        let e = r.run("process.exitCode = 70;\n");
        assert!(!e.threw() && !e.completed());
        // exit 71 propio: no es enlace.
        assert!(!r.run("process.exit(71);\n").link_failed());
        // Una linea `!dz throw` impresa por el programa es salida propia.
        let e = r.run("console.log('!dz throw TypeError');\n");
        assert!(e.ok_with_output() && !e.threw(), "{:?} {}", e.status, e.ctl);
        // `!dz global` impreso no es un global.
        let e = r.run("console.log('!dz global x = 1');\n");
        assert!(!e.has_globals());
        assert!(r.run("globalThis.creado = 1;\n").has_globals());
        // Flags falsos en stderr no cuentan.
        let e = r.run("process.on('exit', () => process.stderr.write('\\n__DIFUZZ_FLAGS__ proxy\\n'));\n");
        assert!(e.flags.is_empty(), "{:?}", e.flags);
        // process.exit pisado: el throw igual termina con 70.
        let e = r.run("process.exit = () => {};\nsetTimeout(() => console.log('tarde'), 10);\nthrow new TypeError('x');\n");
        assert!(e.threw(), "{:?} {}", e.status, e.stdout);
        assert_eq!(e.stdout, "!dz throw TypeError\n");
        // M13: el flag de runtime de `extends`.
        let e = r.run("const v = 1;\nclass A extends v {}\n");
        assert!(e.threw() && e.flags == ["extends"], "{:?}", e.flags);
    }

    /// M11: el programa corre en un cwd propio, vacio en cada ejecucion, con un
    /// entorno minimo.
    #[test]
    fn cada_ejecucion_en_un_cwd_vacio() {
        use super::{Preload, Runner, Status};
        use std::time::Duration;
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m11.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let r = Runner::new(&preload, 9995, Duration::from_secs(20));
        let js = "const fs = require('fs');\nconsole.log(fs.existsSync('x.txt'), process.cwd().endsWith('-w9995-cwd'), process.env.HOME === process.cwd(), Object.keys(process.env).sort().join());\nfs.writeFileSync('x.txt', '1');\n";
        let e1 = r.run(js);
        assert_eq!(e1.status, Status::Exited(0), "{}", e1.stderr);
        assert_eq!(e1.stdout, "false true true HOME,PATH,TMPDIR,TZ\n");
        assert_eq!(r.run(js).stdout, e1.stdout);
        drop(r);
        assert!(!super::run_dir().join(format!("difuzz-{}-w9995-cwd", std::process::id())).exists());
    }

    /// M12: un modulo mudo sin exports se observa con el epilogo (parseado como
    /// modulo), y `--invocar` llama a las funciones exportadas.
    #[test]
    fn carril_b_epilogo_e_invocar_exports() {
        use super::{Preload, Runner, Status};
        use crate::minify::{instrument, SourceKind};
        use std::time::Duration;
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m12.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let r = Runner::new(&preload, 9994, Duration::from_secs(20)).with_kind(SourceKind::Module);
        let mudo = "const o = { a: 1 };\nlet n = await Promise.resolve(2);\nfunction f() { return n; }\n";
        let epi = instrument(mudo, false, SourceKind::Module).expect("un modulo con bindings se observa");
        let e = r.run(&epi);
        assert_eq!(e.status, Status::Exited(0), "{}", e.stderr);
        assert_eq!(e.stdout, "o = {\"a\": 1}\nn = 2\nf = fn\n");
        // `import` no es un binding que observar; un script no parsea como modulo.
        assert!(instrument("import 'node:fs';\n", false, SourceKind::Module).is_none());
        let inv = r.with_invocar(true);
        let e = inv.observing().run("export function f() { return 2; }\nexport const k = 3;\n");
        assert_eq!(e.stdout, "export f = fn\nexport k = 3\nexport f() = 2\n");
    }

    /// M16: los argumentos extra llegan a node.
    #[test]
    fn argumentos_extra_de_node() {
        use super::{Preload, Runner};
        use std::time::Duration;
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m16.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let r = Runner::new(&preload, 9993, Duration::from_secs(20));
        let js = "console.log(process.execArgv.filter((a) => a.startsWith('--stack')).join());\n";
        assert_eq!(r.run(js).stdout, "\n");
        assert_eq!(r.run_with_args(js, &[super::DEEP_STACK]).stdout, format!("{}\n", super::DEEP_STACK));
    }

    /// M20: pid, hrtime, crypto, memoria y uptime son deterministas.
    #[test]
    fn preload_fija_lo_no_determinista() {
        use super::{Preload, Runner, Status};
        use std::time::Duration;
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m20.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let r = Runner::new(&preload, 9992, Duration::from_secs(20));
        let js = "const c = require('crypto'), os = require('os');\nconst t0 = process.hrtime();\nconsole.log(process.pid, process.ppid, process.hrtime(t0), process.hrtime.bigint(), process.uptime());\nconsole.log(c.randomUUID(), crypto.randomUUID(), c.randomBytes(4).toString('hex'), crypto.getRandomValues(new Uint8Array(3)).join(), c.randomInt(10));\nconsole.log(process.memoryUsage().rss, process.memoryUsage.rss(), os.freemem(), os.uptime());\n";
        let a = r.run(js);
        assert_eq!(a.status, Status::Exited(0), "{}", a.stderr);
        assert_eq!(r.run(js).stdout, a.stdout);
        assert!(a.stdout.starts_with("4242 4241 [ 0, 1000000 ] 3000000n 1\n"), "{}", a.stdout);
    }

    /// M21: el mensaje y el stack del throw llegan por el canal, sin prender el
    /// flag `stack` (lo lee el preload, no el programa).
    #[test]
    fn excepcion_con_mensaje_por_el_canal() {
        use super::{Preload, Runner};
        use std::time::Duration;
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let preload = Preload::write_tagged("m21").unwrap();
        let r = Runner::new(&preload, 9991, Duration::from_secs(20));
        let e = r.run("function f(callback) { callback(); }\nf(1);\n");
        assert!(e.threw());
        let exc = e.exc().expect("exc en el canal");
        assert!(exc.starts_with("TypeError: callback is not a function\n    at "), "{exc}");
        assert!(e.flags.is_empty(), "{:?}", e.flags);
    }

    /// M23: los restos de un pid muerto se borran; los de uno vivo no.
    #[test]
    fn limpia_restos_de_pids_muertos() {
        // pid 0 no existe en /proc: es un "muerto" seguro para el test. En un
        // directorio propio: el test no limpia el /tmp real.
        let tmp = std::env::temp_dir().join(format!("difuzz-test-limpieza-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let muerto = tmp.join("difuzz-0-tmp");
        std::fs::create_dir_all(muerto.join("x")).unwrap();
        let vivo = tmp.join(format!("difuzz-{}-tmp", std::process::id()));
        std::fs::create_dir_all(&vivo).unwrap();
        let ajeno = tmp.join("difuzz-test-no-se-toca");
        std::fs::create_dir_all(&ajeno).unwrap();
        std::fs::write(tmp.join("difuzz-0-w1.cjs"), "x").unwrap();
        assert_eq!(super::clean_stale_in(&tmp), 2);
        assert!(!muerto.exists() && !tmp.join("difuzz-0-w1.cjs").exists());
        assert!(vivo.exists() && ajeno.exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// M8: bytes invalidos distintos no colapsan.
    #[test]
    fn utf8_sin_perdida() {
        use super::decode_lossless as d;
        assert_ne!(d(b"\xff"), d(b"\xfe"));
        assert_eq!(d(b"hola \xc3\xb1\n"), "hola \u{f1}\n");
        assert_eq!(d(b"a\xffb"), "a\u{FFFD}ffb");
    }

    #[test]
    fn flags_del_canal_de_control() {
        assert_eq!(ctl_flags("dz\nflags proxy,stack\ndz\n"), ["proxy", "stack"]);
        assert!(ctl_flags("flags \n").is_empty());
        assert!(ctl_flags("nada").is_empty());
    }
}
