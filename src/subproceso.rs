//! M34: minificar en un proceso hijo.
//!
//! En proceso, un desborde de pila real, un abort de C o un OOM del minifier
//! matan la corrida entera (M1 subio la pila a 256 MiB, pero no es un techo).
//! Por defecto (salvo `--en-proceso`), `minify::prepare` corre en un hijo persistente por
//! `Config` (en la practica: uno por worker, mas el de `names_only` si hace
//! falta): el mismo binario con el subcomando oculto `--preparar`. Si el hijo
//! muere (senal, exit raro) o no contesta a tiempo, el caso queda como
//! `Outcome::Panic` con la senal (va a `panic/`), el hijo se re-lanza en el
//! caso siguiente y la corrida sigue.
//!
//! R1-H08 / R2-H04: si no contesta a tiempo o si los hijos del worker pasan el
//! presupuesto de RSS (`configurar_memoria`, sondeo cada `SONDEO_MEMORIA`), el
//! `Outcome::Panic` lleva `PREFIJO_TIMEOUT` o `PREFIJO_MEMORIA` y main lo manda
//! a `recursos/` (`causa_recurso`), no a `panic/`.
//!
//! Protocolo (sin serde, todo medido en bytes):
//! - el hijo saluda con la linea `SALUDO` (lo que venga antes se ignora: en los
//!   tests el hijo es el binario de tests y libtest imprime su cabecera);
//! - pedido: `src <len>\n<fuente>`;
//! - respuesta, una de:
//!   - `ok <dw0> <dw1> <flags|-> <len_original> <len_minificado>\n<original><minificado>`
//!   - `parse <esm 0|1> <script 0|1>\n`
//!   - `codegen\n`
//!   - `panic <len>\n<mensaje>`
//!
//! El hijo corre en su propio grupo de procesos (un Ctrl-C en la terminal no lo
//! mata: si no, cada SIGINT seria un "abort" falso), con un limite de memoria
//! virtual (`LIMITE_MEMORIA`) y, en Linux, `PR_SET_PDEATHSIG` para que no
//! sobreviva a difuzz. Si el padre muere sin mas, el hijo lee EOF y sale.
//!
//! R1-H05, el oraculo sin debug assertions. El binario de difuzz lleva
//! `debug-assertions = true` (Cargo.toml: los ganchos de diagnostico de SWC solo
//! existen asi), pero el paquete npm del minifier no: un `debug_assert!` de oxc
//! (p. ej. `compression_pass.rs:167-190`, eval indirecto) hace panic donde npm
//! devuelve codigo mal compilado. Por eso, ante un `Outcome::Panic`,
//! `minify::prepare` re-minifica el caso con un segundo `Remoto` cuyo ejecutable
//! es el mismo codigo compilado SIN debug assertions:
//!
//!     cargo build --release && cargo build --profile oraculo
//!
//! deja `target/release/difuzz` y `target/oraculo/difuzz` (Cargo.toml,
//! `[profile.oraculo]`). `configurar_oraculo` (una vez, al arrancar) lo busca en
//! `<exe>-oraculo` y `<dir del exe>/../oraculo/<nombre del exe>`, o solo en
//! `DIFUZZ_ORACULO` si esta definida (`DIFUZZ_ORACULO=ninguno` lo apaga), y lo valida con `<oraculo> --preparar --info` (mismo protocolo, mismas
//! versiones de los minifiers, debug_assertions=off). Si no esta o no sirve:
//! una advertencia y el comportamiento de antes (el panic va a `panic/`). El
//! hijo del oraculo es perezoso: se lanza recien con el primer panic del worker.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::backend::{Backend, Minifier};
use crate::minify::{Outcome, Preset, SourceKind};

/// Primera linea del hijo. La version cambia si cambia el protocolo.
const SALUDO: &str = "difuzz-preparar v1";

/// Cuanto puede tardar una minificacion antes de darla por colgada. Generoso a
/// proposito: el binario lleva debug-assertions (Cargo.toml) y un fixture grande
/// tarda segundos. NO SE cual es el maximo real del corpus; ver INFORME.
const TIMEOUT_MINIFICAR: Duration = Duration::from_secs(60);

/// RLIMIT_AS del hijo. La pila del hilo que minifica (256 MiB) es reserva
/// virtual y cuenta aca. Al pasarse, el allocator de Rust aborta ("memory
/// allocation of N bytes failed"): queda como hallazgo con SIGABRT.
/// R2-H04: es el techo; con `configurar_memoria` el limite real es el
/// presupuesto de RSS del worker (menor) y RLIMIT_AS queda de respaldo.
const LIMITE_MEMORIA: u64 = 4 << 30;

/// R2-H04: espacio de direcciones que el hijo reserva sin usarlo (pila de
/// 256 MiB, arenas de malloc, el binario). MEDIDO: un hijo inactivo tiene
/// VmSize 346608 kB y VmPeak 412132 kB con RSS de 10 a 57 MB. RLIMIT_AS =
/// presupuesto de RSS + este margen, para que salte despues del sondeo.
const MARGEN_VIRTUAL: u64 = 1 << 30;

/// R2-H04: piso del presupuesto por worker. MEDIDO: en el corpus completo el
/// pico de RSS de un hijo por caso fue 77 MB (swc) y 67 MB (oxc).
const PRESUPUESTO_MINIMO: u64 = 256 << 20;

/// R2-H04: cada cuanto el padre mira el RSS de los hijos mientras minifica.
/// b2-int: tambien mientras el hijo de analisis analiza (analisis.rs).
pub(crate) const SONDEO_MEMORIA: Duration = Duration::from_millis(50);

/// b2-int (A6 + R2-H04): el presupuesto del worker se reparte entre sus
/// hijos minificadores (config, keep_names, oraculos: `HIJOS_DEL_HILO`) y su
/// hijo de analisis (analisis.rs), que se lleva 1/`PARTE_ANALISIS`. Cada parte
/// se sondea por separado, mientras su hijo trabaja: un minificador no se mata
/// por la memoria que retiene el analisis ni al reves (la causa queda bien
/// atribuida), y la suma de las dos partes es el presupuesto del worker, asi
/// que `jobs` x presupuesto sigue acotando a todos los hijos.
const PARTE_ANALISIS: u64 = 4;

/// b2-int: la parte de los minificadores de un presupuesto de worker.
fn parte_minificadores(p: u64) -> u64 {
    p - p / PARTE_ANALISIS
}

/// b2-int: la parte del hijo de analisis (0: sin configurar, sin sondeo).
pub(crate) fn presupuesto_analisis() -> u64 {
    PRESUPUESTO_WORKER.load(Ordering::SeqCst) / PARTE_ANALISIS
}

/// R1-H08 / R2-H04: primeras palabras del mensaje de un hijo que no termino a
/// tiempo o que paso el limite de memoria. No son panics: `causa_recurso`.
pub const PREFIJO_TIMEOUT: &str = "subproceso: timeout:";
pub const PREFIJO_MEMORIA: &str = "subproceso: memoria:";

/// R2-H04: presupuesto de RSS de los hijos de un worker, en bytes (0: sin
/// configurar, como en los tests: solo RLIMIT_AS = LIMITE_MEMORIA).
static PRESUPUESTO_WORKER: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// R2-H04: pids de los hijos lanzados desde este hilo. Un worker es un
    /// hilo y sus hijos (config, keep_names, oraculos) comparten el presupuesto.
    static HIJOS_DEL_HILO: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// R2-H04: el presupuesto por worker: `--mem-hijo` si vino, si no la mitad de
/// MemTotal repartida entre los workers (la otra mitad queda para node, difuzz
/// y el resto de la maquina), con techo `LIMITE_MEMORIA` y piso
/// `PRESUPUESTO_MINIMO`. Devuelve (bytes, de donde sale).
fn presupuesto(mem_total: Option<u64>, jobs: usize, mem_hijo_mib: Option<u64>) -> (u64, String) {
    if let Some(m) = mem_hijo_mib {
        return (m.saturating_mul(1 << 20), "--mem-hijo".into());
    }
    let Some(total) = mem_total else {
        return (LIMITE_MEMORIA, "sin /proc/meminfo: el techo".into());
    };
    let jobs = jobs.max(1) as u64;
    let p = (total / 2 / jobs).min(LIMITE_MEMORIA);
    if p < PRESUPUESTO_MINIMO {
        let aviso = format!("aviso: MemTotal/2 / {jobs} workers da {} MiB, uso el piso: la suma pasa la mitad de la memoria (baja --jobs)", p >> 20);
        return (PRESUPUESTO_MINIMO, aviso);
    }
    (p, format!("MemTotal/2 / {jobs} workers"))
}

/// MemTotal de `/proc/meminfo`, en bytes.
fn mem_total() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = s.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kb * 1024)
}

/// R2-H04: fija el presupuesto de memoria de los hijos (main, una vez, con
/// los workers reales). Devuelve la linea que main imprime.
pub fn configurar_memoria(jobs: usize, mem_hijo_mib: Option<u64>) -> String {
    let (p, de_donde) = presupuesto(mem_total(), jobs, mem_hijo_mib);
    PRESUPUESTO_WORKER.store(p, Ordering::SeqCst);
    let (min, ana) = (parte_minificadores(p), p / PARTE_ANALISIS);
    format!(
        "memoria de los hijos: {} MiB de RSS por worker ({de_donde}): {} MiB entre sus minificadores (RLIMIT_AS por hijo {} MiB) y {} MiB para su hijo de analisis (RLIMIT_AS {} MiB)",
        p >> 20,
        min >> 20,
        limite_as(min) >> 20,
        ana >> 20,
        limite_as(ana) >> 20
    )
}

/// R2-H04: RLIMIT_AS para un presupuesto de RSS (0: el techo de siempre).
pub(crate) fn limite_as(presupuesto: u64) -> u64 {
    if presupuesto == 0 {
        LIMITE_MEMORIA
    } else {
        presupuesto.saturating_add(MARGEN_VIRTUAL)
    }
}

/// R1-H08 / R2-H04: `Some("timeout")` o `Some("memoria")` si el mensaje de un
/// `Outcome::Panic` es en realidad un hijo que no termino o que se quedo sin
/// memoria. Mira la primera linea: el oraculo agrega las suyas despues.
pub fn causa_recurso(msg: &str) -> Option<&'static str> {
    if msg.starts_with(PREFIJO_TIMEOUT) {
        Some("timeout")
    } else if msg.starts_with(PREFIJO_MEMORIA) {
        Some("memoria")
    } else {
        None
    }
}

/// RSS de un proceso en bytes (`/proc/<pid>/statm`, campo 2 en paginas); 0 si
/// ya no esta.
pub(crate) fn rss(pid: u32) -> u64 {
    // SAFETY: sysconf con una constante; no toca memoria.
    let pagina = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
    std::fs::read_to_string(format!("/proc/{pid}/statm"))
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |p| p * pagina)
}

/// R2-H04: RSS de todos los hijos vivos de este hilo.
fn rss_del_hilo() -> u64 {
    HIJOS_DEL_HILO.try_with(|h| h.borrow().iter().map(|&p| rss(p)).sum()).unwrap_or(0)
}

/// Cuanto stderr del hijo se guarda para el mensaje del hallazgo (la cola).
const COLA_STDERR: usize = 4096;

static ACTIVO: AtomicBool = AtomicBool::new(false);

/// Por defecto (sin `--en-proceso`): desde aca, `Preset::build` y `Config::con_nombres`
/// devuelven configs que minifican en un hijo.
pub fn activar() {
    ACTIVO.store(true, Ordering::SeqCst);
}

pub fn activo() -> bool {
    ACTIVO.load(Ordering::SeqCst)
}

/// Que programa lanzar como hijo. En produccion, este mismo binario con
/// `--preparar`; en los tests, el binario de tests con un filtro (ver tests).
#[derive(Clone)]
struct Lanzador {
    exe: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

/// Argumentos del subcomando oculto: los que necesita `serve` para armar la
/// misma config que tendria el worker.
fn args_config(minifier: Minifier, preset: Preset, kind: SourceKind, keep_names: bool) -> Vec<String> {
    let mut v = vec![
        "--minifier".to_string(),
        minifier.name().to_string(),
        "--config".to_string(),
        preset.name().to_string(),
        "--carril".to_string(),
        kind.carril().to_string(),
    ];
    if keep_names {
        v.push("--keep-names".into());
    }
    v
}

/// Backend que delega en un hijo. Uno por `Config`: el hijo es persistente y se
/// re-lanza si muere.
pub struct Remoto {
    lanzador: Lanzador,
    /// Si el hijo no se puede lanzar, se minifica en proceso (con aviso).
    /// `None` (el oraculo, R1-H05): no hay respaldo en proceso (este proceso
    /// lleva debug assertions) y el caso queda como panic, con el motivo.
    local: Option<Box<dyn Fn() -> Box<dyn Backend>>>,
    hijo: Mutex<Option<Hijo>>,
    timeout: Duration,
    /// R2-H04: presupuesto de RSS (bytes) de los hijos del worker; `None`: el
    /// global (`configurar_memoria`). 0: sin sondeo.
    presupuesto: Option<u64>,
}

struct Hijo {
    child: Child,
    /// R2-H04: el RLIMIT_AS con el que se lanzo (para el mensaje).
    limite_as: u64,
    stdin: ChildStdin,
    rx: mpsc::Receiver<std::io::Result<Outcome>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_hilo: Option<std::thread::JoinHandle<()>>,
}

impl Remoto {
    pub fn new(minifier: Minifier, preset: Preset, kind: SourceKind, keep_names: bool) -> Remoto {
        // M3: `/proc/self/exe` (Linux), el codigo de esta corrida aunque recompilen.
        let exe = crate::binario::propio();
        let mut args = vec!["--preparar".to_string()];
        args.extend(args_config(minifier, preset, kind, keep_names));
        Remoto {
            lanzador: Lanzador { exe, args, env: Vec::new() },
            local: Some(Box::new(move || local_backend(minifier, preset, kind, keep_names))),
            hijo: Mutex::new(None),
            timeout: TIMEOUT_MINIFICAR,
            presupuesto: None,
        }
    }

    /// R1-H05: el mismo hijo, pero con el ejecutable del oraculo (sin debug
    /// assertions). Si no se puede lanzar NO se minifica en proceso (este proceso
    /// lleva los asserts): el caso queda como panic, con el motivo.
    fn oraculo(exe: PathBuf, minifier: Minifier, preset: Preset, kind: SourceKind, keep_names: bool) -> Remoto {
        let mut args = vec!["--preparar".to_string()];
        args.extend(args_config(minifier, preset, kind, keep_names));
        Remoto {
            lanzador: Lanzador { exe, args, env: Vec::new() },
            local: None,
            hijo: Mutex::new(None),
            timeout: TIMEOUT_MINIFICAR,
            presupuesto: None,
        }
    }

    /// R2-H04: el presupuesto efectivo (0: sin limite de RSS).
    fn presupuesto(&self) -> u64 {
        // b2-int: la parte de los minificadores (el resto es del hijo de analisis).
        self.presupuesto.unwrap_or_else(|| parte_minificadores(PRESUPUESTO_WORKER.load(Ordering::SeqCst)))
    }

    fn lanzar(&self) -> std::io::Result<Hijo> {
        // R1-H18: si el binario (o el oraculo) cambio desde el arranque, no se lanza.
        crate::binario::comprobar().map_err(std::io::Error::other)?;
        // M3: el oraculo por el fd abierto al arrancar (`/proc/self/fd/N`): el inodo
        // validado, aunque su ruta ya apunte a otro archivo.
        let mut cmd = Command::new(crate::binario::para_lanzar(&self.lanzador.exe));
        cmd.args(&self.lanzador.args)
            .envs(self.lanzador.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let limite_as = limite_as(self.presupuesto());
        preparar_hijo(&mut cmd, limite_as);
        let mut child = cmd.spawn()?;
        let pid = child.id();
        let _ = HIJOS_DEL_HILO.try_with(|h| h.borrow_mut().push(pid));
        let stdin = child.stdin.take().expect("stdin pedido como piped");
        let stdout = child.stdout.take().expect("stdout pedido como piped");
        let stderr_pipe = child.stderr.take().expect("stderr pedido como piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || leer_respuestas(BufReader::new(stdout), tx));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let buf = Arc::clone(&stderr);
        let stderr_hilo = std::thread::spawn(move || juntar_cola(stderr_pipe, buf));
        Ok(Hijo { child, limite_as, stdin, rx, stderr, stderr_hilo: Some(stderr_hilo) })
    }
}

/// R1-H05 / R1-H18: comienzo del panic de un oraculo que no se pudo lanzar
/// (`minify::respaldo` no lo presenta como "tambien falla (es real)").
pub const ORACULO_NO_LANZA: &str = "subproceso: no pude lanzar el oraculo sin debug assertions";

/// R1-H05: ruta del oraculo validado, o `None`. La fija `configurar_oraculo`
/// (main, al arrancar); sin llamarla (tests, `--emitir`) no hay oraculo.
static ORACULO: OnceLock<Option<PathBuf>> = OnceLock::new();

/// R1-H05: si `configurar_oraculo` encontro uno.
pub fn hay_oraculo() -> bool {
    ORACULO.get().is_some_and(Option::is_some)
}

/// R1-H05: el backend de respaldo para una config, si hay oraculo.
pub fn oraculo(minifier: Minifier, preset: Preset, kind: SourceKind, keep_names: bool) -> Option<Box<dyn Backend>> {
    let exe = ORACULO.get()?.clone()?;
    Some(Box::new(Remoto::oraculo(exe, minifier, preset, kind, keep_names)))
}

/// R1-H05: lo que imprime `difuzz --preparar --info`. El padre exige la misma
/// linea con `debug_assertions=off`: mismo protocolo y mismos minifiers.
pub fn info(debug_assertions: bool) -> String {
    // 7a: las versiones de todos los backends de produccion, en su orden.
    let versiones: Vec<&str> = Minifier::TODOS.iter().map(|m| m.version()).collect();
    format!(
        "{SALUDO}; difuzz {}; {}; debug_assertions={}",
        env!("CARGO_PKG_VERSION"),
        versiones.join("; "),
        if debug_assertions { "on" } else { "off" }
    )
}

/// R1-H05: donde buscar el oraculo, en orden. `DIFUZZ_ORACULO` (no vacio) es
/// el unico candidato: si no sirve, aviso y sin oraculo (asi tambien se apaga,
/// p. ej. `DIFUZZ_ORACULO=ninguno`), nunca otro en silencio.
fn candidatos_oraculo(exe: &std::path::Path, env: Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(e) = env.filter(|e| !e.is_empty()) {
        v.push(PathBuf::from(e));
        return v;
    }
    let mut hermano = exe.as_os_str().to_owned();
    hermano.push("-oraculo");
    v.push(PathBuf::from(hermano));
    if let (Some(dir), Some(nombre)) = (exe.parent().and_then(|d| d.parent()), exe.file_name()) {
        v.push(dir.join("oraculo").join(nombre));
    }
    v
}

/// R1-H05: corre `<exe> --preparar --info` y compara con lo esperado.
fn validar_oraculo(exe: &std::path::Path) -> Result<(), String> {
    let out = Command::new(exe)
        .args(["--preparar", "--info"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("no pude correrlo: {e}"))?;
    let dice = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let espera = info(false);
    if out.status.success() && dice == espera {
        Ok(())
    } else if dice == info(true) {
        Err("tiene debug assertions (compilalo con --profile oraculo)".into())
    } else {
        Err(format!("no es el mismo difuzz: dice {dice:?}, espero {espera:?} (recompilalo)"))
    }
}

/// R1-H05: busca y valida el oraculo una vez. `Ok(Some(ruta))`: se usa;
/// `Ok(None)`: este binario ya no tiene debug assertions (no hace falta);
/// `Err(aviso)`: no hay oraculo usable y los panics van a `panic/` como antes
/// (main imprime el aviso, una vez). Un oraculo mas viejo que este binario se
/// usa igual, con el aviso en `Ok` via `aviso_viejo`.
pub fn configurar_oraculo() -> Result<Option<PathBuf>, String> {
    if !cfg!(debug_assertions) {
        let _ = ORACULO.set(None);
        return Ok(None);
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("difuzz"));
    let r = elegir_oraculo(&exe, std::env::var_os("DIFUZZ_ORACULO"));
    let _ = ORACULO.set(r.as_ref().ok().cloned());
    r.map(Some)
}

fn elegir_oraculo(exe: &std::path::Path, env: Option<std::ffi::OsString>) -> Result<PathBuf, String> {
    let explicito = env.as_ref().is_some_and(|e| !e.is_empty());
    let mut motivos = Vec::new();
    for c in candidatos_oraculo(exe, env) {
        if !c.is_file() {
            // El de DIFUZZ_ORACULO tiene que existir: el aviso lo nombra.
            if explicito {
                motivos.push(format!("DIFUZZ_ORACULO={}: no existe", c.display()));
            }
            continue;
        }
        match validar_oraculo(&c) {
            Ok(()) => return Ok(c),
            Err(e) => motivos.push(format!("{}: {e}", c.display())),
        }
    }
    let probe = candidatos_oraculo(exe, None).iter().map(|c| c.display().to_string()).collect::<Vec<_>>().join(", ");
    Err(format!(
        "no hay oraculo sin debug assertions{}: un panic del minifier va a panic/ aunque sea solo un debug_assert \
         (npm no los tiene). Compilalo con `cargo build --profile oraculo` (busco en DIFUZZ_ORACULO, {probe})",
        if motivos.is_empty() { String::new() } else { format!(" ({})", motivos.join("; ")) }
    ))
}

/// R1-H05: aviso si el oraculo es mas viejo que este binario (puede ser de
/// otro codigo con las mismas versiones de los minifiers).
pub fn aviso_viejo(oraculo: &std::path::Path) -> Option<String> {
    let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let exe = std::env::current_exe().ok()?;
    (mtime(oraculo)? < mtime(&exe)?).then(|| {
        format!("el oraculo {} es mas viejo que este binario: si cambio el codigo, recompilalo (cargo build --profile oraculo)", oraculo.display())
    })
}

/// La config del worker, en proceso (lo que el hijo arma en `serve`).
fn local_backend(minifier: Minifier, preset: Preset, kind: SourceKind, keep_names: bool) -> Box<dyn Backend> {
    if keep_names {
        minifier.build_con_nombres(preset, kind)
    } else {
        minifier.build(preset, kind)
    }
}

/// Grupo propio, limite de memoria y (Linux) morir con el padre.
pub(crate) fn preparar_hijo(cmd: &mut Command, limite_as: u64) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    // SAFETY: entre fork y exec solo se llaman setrlimit y prctl, que son
    // async-signal-safe (syscalls directas, sin allocar).
    unsafe {
        cmd.pre_exec(move || {
            let lim = libc::rlimit {
                rlim_cur: limite_as as libc::rlim_t,
                rlim_max: limite_as as libc::rlim_t,
            };
            // Si falla (p. ej. el limite duro ya es menor), el hijo corre igual.
            libc::setrlimit(libc::RLIMIT_AS, &lim);
            #[cfg(target_os = "linux")]
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
}

impl Backend for Remoto {
    fn prepare(&self, src: &str) -> Outcome {
        let mut slot = self.hijo.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            match self.lanzar() {
                Ok(h) => *slot = Some(h),
                Err(e) => {
                    drop(slot);
                    let Some(local) = &self.local else {
                        // El oraculo (R1-H05): no minifica en proceso. Con R1-H18 el motivo
                        // es que el ejecutable cambio a mitad de corrida (binario.rs).
                        eprintln!(
                            "subproceso: no pude lanzar el oraculo {}: {e}; el caso queda en panic/ (el oraculo no tiene respaldo en proceso)",
                            self.lanzador.exe.display()
                        );
                        return Outcome::Panic(format!("{ORACULO_NO_LANZA}: {e}"));
                    };
                    eprintln!("subproceso: no pude lanzar {}: {e}; minifico en proceso", self.lanzador.exe.display());
                    return local().prepare(src);
                }
            }
        }
        let h = slot.as_mut().expect("recien lanzado");
        let enviado = writeln!(h.stdin, "src {}", src.len())
            .and_then(|()| h.stdin.write_all(src.as_bytes()))
            .and_then(|()| h.stdin.flush());
        let r = match enviado {
            Ok(()) => self.esperar(h),
            // EPIPE: el hijo ya no esta. Se diagnostica igual que una muerte.
            Err(e) => Ok(Ok(Err(e))),
        };
        let r = match r {
            Ok(r) => r,
            // R2-H04: paso el presupuesto de memoria del worker.
            Err((rss_hilo, rss_hijo, limite)) => {
                let h = slot.take().expect("hijo presente");
                return Outcome::Panic(h.muerte(Causa::Memoria { rss_hilo, rss_hijo, limite }));
            }
        };
        match r {
            Ok(Ok(o)) => o,
            // R1-H07: una respuesta ilegible no es una muerte del hijo (antes salia
            // como "murio por senal 9": la senal la mandaba muerte() al matarlo).
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::InvalidData => {
                let h = slot.take().expect("hijo presente");
                Outcome::Panic(h.muerte(Causa::Protocolo(e.to_string())))
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let h = slot.take().expect("hijo presente");
                Outcome::Panic(h.muerte(Causa::Murio))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let h = slot.take().expect("hijo presente");
                Outcome::Panic(h.muerte(Causa::Timeout(self.timeout)))
            }
        }
    }
}

impl Remoto {
    /// Espera la respuesta del hijo hasta `timeout`. R2-H04: cada
    /// `SONDEO_MEMORIA` suma el RSS de los hijos de este worker; si pasa el
    /// presupuesto, `Err((rss del worker, rss del hijo, presupuesto))`.
    #[allow(clippy::type_complexity)]
    fn esperar(&self, h: &Hijo) -> Result<Result<std::io::Result<Outcome>, mpsc::RecvTimeoutError>, (u64, u64, u64)> {
        let presupuesto = self.presupuesto();
        let t0 = Instant::now();
        loop {
            let falta = self.timeout.saturating_sub(t0.elapsed());
            if falta.is_zero() {
                return Ok(Err(mpsc::RecvTimeoutError::Timeout));
            }
            let tramo = if presupuesto == 0 { falta } else { falta.min(SONDEO_MEMORIA) };
            match h.rx.recv_timeout(tramo) {
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                r => return Ok(r),
            }
            if presupuesto > 0 {
                let total = rss_del_hilo();
                if total > presupuesto {
                    return Err((total, rss(h.child.id()), presupuesto));
                }
            }
        }
    }
}

/// Por que se da por perdido al hijo (el mensaje del hallazgo lo dice).
enum Causa {
    /// EOF o pipe roto: murio solo (senal o exit).
    Murio,
    Timeout(Duration),
    /// R2-H04: el RSS de los hijos del worker paso el presupuesto (bytes).
    Memoria { rss_hilo: u64, rss_hijo: u64, limite: u64 },
    /// R1-H07: contesto algo ilegible; lo mata difuzz.
    Protocolo(String),
}

impl Hijo {
    /// Mata (si sigue vivo), cosecha y arma el mensaje del hallazgo.
    fn muerte(mut self, causa: Causa) -> String {
        let _ = self.child.kill();
        let st = self.child.wait();
        if let Some(t) = self.stderr_hilo.take() {
            let _ = t.join();
        }
        let cola = String::from_utf8_lossy(&self.stderr.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
        let mut msg = match (causa, st) {
            // R1-H08: no es un panic; `causa_recurso` lo reconoce por el prefijo.
            (Causa::Timeout(t), _) => format!("{PREFIJO_TIMEOUT} el minifier no termino en {:.1} s (lento o colgado; hijo matado)", t.as_secs_f64()),
            (Causa::Memoria { rss_hilo, rss_hijo, limite }, _) => format!(
                "{PREFIJO_MEMORIA} los hijos minificadores del worker pasaron su presupuesto de RSS: {} MiB > {} MiB (este hijo {} MiB; hijo matado)",
                rss_hilo >> 20,
                limite >> 20,
                rss_hijo >> 20
            ),
            (Causa::Protocolo(e), _) => format!("subproceso: error de protocolo con el hijo ({e}); hijo matado"),
            // R2-H04: el allocator no pudo reservar (RLIMIT_AS, el respaldo del sondeo).
            (Causa::Murio, Ok(st)) if cola.contains("memory allocation of") => {
                format!("{PREFIJO_MEMORIA} el minifier no pudo reservar memoria (RLIMIT_AS {} MiB); {}", self.limite_as >> 20, describir(st))
            }
            (Causa::Murio, Ok(st)) => describir(st),
            (Causa::Murio, Err(e)) => format!("subproceso: el hijo murio y no pude cosecharlo: {e}"),
        };
        if !cola.trim().is_empty() {
            msg.push_str("\nstderr del hijo:\n");
            msg.push_str(cola.trim_end());
        }
        msg
    }
}

impl Drop for Hijo {
    fn drop(&mut self) {
        // Sin pedido en curso el hijo esta leyendo stdin: con kill o con EOF
        // termina igual. kill no espera a que llegue el EOF.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let pid = self.child.id();
        let _ = HIJOS_DEL_HILO.try_with(|h| h.borrow_mut().retain(|&p| p != pid));
    }
}

/// "murio por senal 6 (SIGABRT)" o "salio con codigo N".
pub(crate) fn describir(st: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (st.signal(), st.code()) {
        (Some(s), _) => format!("subproceso: el minifier murio por senal {s} ({})", nombre_senal(s)),
        (None, Some(c)) => format!("subproceso: el minifier salio con codigo {c} sin contestar"),
        (None, None) => "subproceso: el minifier termino sin codigo ni senal".into(),
    }
}

fn nombre_senal(s: i32) -> &'static str {
    match s {
        libc::SIGABRT => "SIGABRT",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGBUS => "SIGBUS",
        libc::SIGILL => "SIGILL",
        libc::SIGFPE => "SIGFPE",
        libc::SIGKILL => "SIGKILL",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGTERM => "SIGTERM",
        _ => "otra",
    }
}

/// Hilo lector: saludo y despues una respuesta por pedido. Al primer error
/// (EOF: el hijo murio) lo manda y termina.
fn leer_respuestas(mut r: impl BufRead, tx: mpsc::Sender<std::io::Result<Outcome>>) {
    let mut linea = String::new();
    loop {
        linea.clear();
        match r.read_line(&mut linea) {
            Ok(0) => {
                let _ = tx.send(Err(std::io::ErrorKind::UnexpectedEof.into()));
                return;
            }
            Ok(_) if linea.trim_end() == SALUDO => break,
            Ok(_) => {}
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        }
    }
    loop {
        let o = leer_outcome(&mut r);
        let fin = o.is_err();
        if tx.send(o).is_err() || fin {
            return;
        }
    }
}

/// Junta el stderr del hijo, quedandose con la cola.
pub(crate) fn juntar_cola(mut pipe: impl Read, buf: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
                b.extend_from_slice(&chunk[..n]);
                if b.len() > COLA_STDERR {
                    let sobra = b.len() - COLA_STDERR;
                    b.drain(..sobra);
                }
            }
        }
    }
}

fn invalido(msg: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

fn leer_bytes(r: &mut impl BufRead, n: usize) -> std::io::Result<String> {
    let mut v = vec![0u8; n];
    r.read_exact(&mut v)?;
    String::from_utf8(v).map_err(|e| invalido(format!("utf-8: {e}")))
}

fn leer_outcome(r: &mut impl BufRead) -> std::io::Result<Outcome> {
    let mut cab = String::new();
    if r.read_line(&mut cab)? == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    let campos: Vec<&str> = cab.trim_end_matches('\n').split(' ').collect();
    let num = |s: &str| s.parse::<usize>().map_err(|_| invalido(format!("cabecera: {cab:?}")));
    match campos[..] {
        // b2-int (R1-H13): el estado del analisis NO viaja por este protocolo.
        // Con oxc el hijo no corre los chequeos (`activar_checks_afuera`): el
        // estado lo trae `analisis::Analizado` desde el hijo de analisis. Con swc
        // el analisis es sobre su propio AST (limpio) y un fallo ya cruza como
        // la marca `FALLO_ANALISIS`, de la que Analizado deriva `Perdido`.
        ["ok", dw0, dw1, flags, lo, lm] => {
            let (dw0, dw1, lo, lm) = (num(dw0)?, num(dw1)?, num(lo)?, num(lm)?);
            let static_flags = if flags == "-" { Vec::new() } else { flags.split(',').map(intern_flag).collect() };
            let original = leer_bytes(r, lo)?;
            let minified = leer_bytes(r, lm)?;
            Ok(Outcome::Ok { original, minified, static_flags, dead_writes: (dw0, dw1), analisis: Default::default() })
        }
        ["parse", esm, script] => Ok(Outcome::ParseError { esm: esm == "1", script: script == "1" }),
        ["codegen"] => Ok(Outcome::CodegenError),
        ["panic", n] => Ok(Outcome::Panic(leer_bytes(r, num(n)?)?)),
        _ => Err(invalido(format!("cabecera: {cab:?}"))),
    }
}

fn escribir_outcome(w: &mut impl Write, o: &Outcome) -> std::io::Result<()> {
    match o {
        Outcome::Ok { original, minified, static_flags, dead_writes, .. } => {
            let flags = if static_flags.is_empty() { "-".to_string() } else { static_flags.join(",") };
            writeln!(w, "ok {} {} {flags} {} {}", dead_writes.0, dead_writes.1, original.len(), minified.len())?;
            w.write_all(original.as_bytes())?;
            w.write_all(minified.as_bytes())
        }
        Outcome::ParseError { esm, script } => writeln!(w, "parse {} {}", u8::from(*esm), u8::from(*script)),
        Outcome::CodegenError => w.write_all(b"codegen\n"),
        Outcome::Panic(m) => {
            writeln!(w, "panic {}", m.len())?;
            w.write_all(m.as_bytes())
        }
    }
}

/// Los flags estaticos son `&'static str` (checks::static_flags). Del otro lado
/// del pipe llegan como texto: se vuelven a la constante. Un flag nuevo que no
/// este aca se filtra (Box::leak) en vez de perderse; el test
/// `flags_estaticos_conocidos` avisa si falta alguno.
const FLAGS_ESTATICOS: [&str; 6] = ["callee", "redefine_global", "coercion", "with", "global_conocido", "extends"];

pub(crate) fn intern_flag(s: &str) -> &'static str {
    FLAGS_ESTATICOS
        .iter()
        .chain([&crate::analisis::FALLO_ANALISIS])
        .find(|f| **f == s)
        .copied()
        .unwrap_or_else(|| Box::leak(s.to_string().into_boxed_str()))
}

/// El subcomando oculto `difuzz --preparar --minifier M --config C --carril K
/// [--keep-names]`. Devuelve el exit code.
pub fn serve(args: &[String]) -> i32 {
    // R1-H05: `--preparar --info`, la identidad que valida `configurar_oraculo`.
    if args == ["--info"] {
        println!("{}", info(cfg!(debug_assertions)));
        return 0;
    }
    // El hook de main no llega aca (el subcomando sale antes): sin esto cada
    // panic del minifier se imprimiria en stderr, y se pierde la ubicacion (M22).
    std::panic::set_hook(Box::new(crate::minify::record_panic_location));
    // A6/R1-H12: oxc no corre los chequeos con SWC en este hijo (analisis.rs).
    crate::analisis::activar_checks_afuera();
    let salida = std::io::BufWriter::new(std::io::stdout());
    serve_con_pila(args, crate::WORKER_STACK, BufReader::new(std::io::stdin()), salida)
}

fn serve_con_pila(args: &[String], pila: usize, entrada: impl BufRead + Send, salida: impl Write + Send) -> i32 {
    // 7b: el backend no tiene default (antes `Swc` si faltaba): el padre
    // siempre lo pasa (args_config).
    let (mut minifier, mut preset, mut kind, mut keep) = (None, Preset::CarrilA, SourceKind::Script, false);
    let mut i = 0;
    while i < args.len() {
        let v = args.get(i + 1).map(String::as_str).unwrap_or_default();
        let ok = match args[i].as_str() {
            "--minifier" => Minifier::parse(v).map(|m| minifier = Some(m)).is_some(),
            "--config" => Preset::parse(v).map(|p| preset = p).is_some(),
            "--carril" => SourceKind::parse(v).map(|k| kind = k).is_some(),
            "--keep-names" => {
                keep = true;
                i += 1;
                continue;
            }
            _ => false,
        };
        if !ok {
            eprintln!("--preparar: argumento invalido {}", args[i]);
            return 2;
        }
        i += 2;
    }
    let Some(minifier) = minifier else {
        eprintln!("--preparar: falta --minifier");
        return 2;
    };
    // Misma pila que un worker (M1): un fuente que pasa en proceso pasa aca.
    let r = std::thread::scope(|s| {
        std::thread::Builder::new()
            .name("preparar".into())
            .stack_size(pila)
            .spawn_scoped(s, move || bucle(local_backend(minifier, preset, kind, keep), entrada, salida))
            .map(|h| h.join())
    });
    match r {
        Ok(Ok(Ok(()))) => 0,
        Ok(Ok(Err(e))) => {
            eprintln!("--preparar: {e}");
            1
        }
        _ => 1,
    }
}

fn bucle(backend: Box<dyn Backend>, mut entrada: impl BufRead, mut salida: impl Write) -> std::io::Result<()> {
    let cfg = crate::minify::Config::from_backend(backend);
    writeln!(salida, "{SALUDO}")?;
    salida.flush()?;
    let mut cab = String::new();
    loop {
        cab.clear();
        if entrada.read_line(&mut cab)? == 0 {
            return Ok(());
        }
        let n: usize = cab
            .trim_end()
            .strip_prefix("src ")
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| invalido(format!("pedido: {cab:?}")))?;
        let src = leer_bytes(&mut entrada, n)?;
        let o = crate::minify::prepare(&src, &cfg);
        escribir_outcome(&mut salida, &o)?;
        salida.flush()?;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn ida_y_vuelta(o: &Outcome) -> Outcome {
        let mut v = Vec::new();
        escribir_outcome(&mut v, o).unwrap();
        leer_outcome(&mut &v[..]).unwrap()
    }

    #[test]
    fn protocolo_ida_y_vuelta() {
        let o = Outcome::Ok {
            original: "a\nb ñ\n".into(),
            minified: "".into(),
            static_flags: vec!["with", "extends"],
            dead_writes: (1, 2),
            analisis: crate::checks::Analisis::de_codigo("rlp").unwrap(),
        };
        let Outcome::Ok { original, minified, static_flags, dead_writes, analisis } = ida_y_vuelta(&o) else { panic!() };
        assert_eq!((original.as_str(), minified.as_str(), dead_writes), ("a\nb ñ\n", "", (1, 2)));
        assert_eq!(static_flags, ["with", "extends"]);
        // b2-int (R1-H13): el estado del analisis no cruza este pipe (lo trae
        // analisis::Analizado del hijo de analisis): del hijo minificador sale limpio.
        assert!(analisis.limpio());
        let Outcome::Ok { analisis, minified, .. } = leer_outcome(&mut &b"ok 0 0 - 0 1\nx"[..]).unwrap() else { panic!() };
        assert!(analisis.limpio() && minified == "x");
        assert!(leer_outcome(&mut &b"ok 0 0 - 0 0 lll\n"[..]).is_err());
        assert!(matches!(ida_y_vuelta(&Outcome::ParseError { esm: true, script: false }), Outcome::ParseError { esm: true, script: false }));
        assert!(matches!(ida_y_vuelta(&Outcome::CodegenError), Outcome::CodegenError));
        let Outcome::Panic(m) = ida_y_vuelta(&Outcome::Panic("x\ny".into())) else { panic!() };
        assert_eq!(m, "x\ny");
        // Una cabecera corrupta es error, no un Outcome inventado.
        assert!(leer_outcome(&mut &b"ok 1\n"[..]).is_err());
        assert!(leer_outcome(&mut &b""[..]).is_err());
    }

    /// Todos los flags que puede dar checks::static_flags vuelven a su constante.
    /// 9d (R1-H17): antes solo se miraba lo que daba un programa de ejemplo, y
    /// un flag nuevo que ese programa no dispara pasaba sin aviso. Ahora la
    /// lista sale del fuente de `checks::static_flags` (cada `out.push("...")`)
    /// y tiene que ser exactamente FLAGS_ESTATICOS.
    /// 09-oct: los flags estaticos de oxc viven en su propia funcion
    /// (`checks::static_flags_oxc`, la pide `Politica::flags_estaticos`); las
    /// dos cruzan el pipe del hijo de analisis y se miran las dos.
    #[test]
    fn flags_estaticos_conocidos() {
        let fuente = include_str!("checks.rs");
        let mut en_checks: Vec<&str> = Vec::new();
        for f in ["pub fn static_flags(", "pub fn static_flags_oxc("] {
            let cuerpo = &fuente[fuente.find(f).unwrap_or_else(|| panic!("checks.rs: falta {f}"))..];
            let cuerpo = &cuerpo[..cuerpo.find("\n}\n").unwrap_or_else(|| panic!("checks.rs: fin de {f}"))];
            let antes = en_checks.len();
            en_checks.extend(cuerpo.match_indices("out.push(\"").map(|(i, p)| {
                let resto = &cuerpo[i + p.len()..];
                &resto[..resto.find('"').expect("literal cerrado")]
            }));
            // Otra forma de agregar flags (extend, insert, push de algo que no es un
            // literal) no la ve este test: que falle y se actualice.
            assert_eq!(cuerpo.matches("out.").count(), en_checks.len() - antes, "{f} agrega flags de otra forma:\n{cuerpo}");
        }
        en_checks.sort_unstable();
        let mut lista = FLAGS_ESTATICOS.to_vec();
        lista.sort_unstable();
        assert_eq!(en_checks, lista, "los flags de checks::static_flags y static_flags_oxc no son FLAGS_ESTATICOS");
        // Y el programa de ejemplo los dispara todos (el fuente no miente).
        let p = crate::backend::swc::parse_for(
            "with (o) {} arguments.callee; function g(NaN) {} var o = { valueOf() {} }; window; class A extends 1 {}",
            SourceKind::Script,
        )
        .unwrap();
        let mut fl = crate::checks::static_flags(&p);
        fl.extend(crate::checks::static_flags_oxc(&p));
        assert_eq!(fl.len(), FLAGS_ESTATICOS.len(), "{fl:?}");
        for f in fl {
            assert!(FLAGS_ESTATICOS.contains(&f), "falta {f} en FLAGS_ESTATICOS");
        }
    }

    /// `serve` en un hilo con pipes en memoria: misma salida que en proceso.
    #[test]
    fn serve_contesta_igual_que_en_proceso() {
        let fuentes = ["var x = 1 + 2; console.log(x);\n", "let = ;", "with (o) { x; }\n"];
        let mut entrada = Vec::new();
        for s in fuentes {
            entrada.extend_from_slice(format!("src {}\n{s}", s.len()).as_bytes());
        }
        let mut salida = Vec::new();
        let args: Vec<String> = args_config(Minifier::Swc, Preset::CarrilA, SourceKind::Script, false);
        assert_eq!(serve_con_pila(&args, 8 << 20, &entrada[..], &mut salida), 0);
        let mut r = &salida[..];
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        assert_eq!(l.trim_end(), SALUDO);
        let local = crate::minify::Preset::CarrilA.build(Minifier::Swc, SourceKind::Script);
        for s in fuentes {
            let remoto = leer_outcome(&mut r).unwrap();
            match (remoto, crate::minify::prepare(s, &local)) {
                (Outcome::Ok { original: o1, minified: m1, static_flags: f1, dead_writes: d1, analisis: a1 }, Outcome::Ok { original: o2, minified: m2, static_flags: f2, dead_writes: d2, analisis: a2 }) => {
                    assert_eq!((o1, m1, f1, d1, a1), (o2, m2, f2, d2, a2));
                }
                (Outcome::ParseError { esm: e1, script: s1 }, Outcome::ParseError { esm: e2, script: s2 }) => assert_eq!((e1, s1), (e2, s2)),
                _ => panic!("distinto para {s:?}"),
            }
        }
    }

    /// El hijo de los tests: el binario de tests filtrado a este test, con la
    /// config en una variable de entorno. Sin la variable no hace nada (pasa).
    #[test]
    fn servidor_hijo() {
        let Ok(cfg) = std::env::var("DIFUZZ_TEST_HIJO") else {
            return;
        };
        let args: Vec<String> = cfg.split(' ').map(String::from).collect();
        std::panic::set_hook(Box::new(crate::minify::record_panic_location));
        // R1-H08 / R2-H04: un hijo lento o que come memoria, a pedido del test.
        let num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<usize>().ok());
        if let Some(mib) = num("DIFUZZ_TEST_COMER_MIB") {
            std::mem::forget(std::hint::black_box(vec![1u8; mib << 20]));
        }
        if let Some(mib) = num("DIFUZZ_TEST_RESERVAR_MIB") {
            std::mem::forget(std::hint::black_box(Vec::<u8>::with_capacity(mib << 20)));
        }
        if let Some(ms) = num("DIFUZZ_TEST_DORMIR_MS") {
            std::thread::sleep(Duration::from_millis(ms as u64));
        }
        // Pila chica a proposito: un anidamiento moderado la desborda y el hijo
        // aborta de verdad (SIGABRT), como lo haria uno de 256 MiB con mas anidamiento.
        let salida = std::io::BufWriter::new(std::io::stdout());
        let code = serve_con_pila(&args, 1 << 20, BufReader::new(std::io::stdin()), salida);
        std::process::exit(code);
    }

    fn remoto_de_test(minifier: Minifier, kind: SourceKind) -> Remoto {
        remoto_de_test_con(minifier, kind, &[], Duration::from_secs(30), Some(0))
    }

    /// R1-H08 / R2-H04: con variables para el hijo, timeout y presupuesto propios.
    /// 9c: tambien lo usa e2e_tests (el hijo minificador real, para recursos/).
    pub(crate) fn remoto_de_test_con(minifier: Minifier, kind: SourceKind, env: &[(&str, &str)], timeout: Duration, presupuesto: Option<u64>) -> Remoto {
        let exe = std::env::current_exe().unwrap();
        let cfg = args_config(minifier, Preset::CarrilA, kind, false).join(" ");
        let mut env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        env.push(("DIFUZZ_TEST_HIJO".into(), cfg));
        Remoto {
            lanzador: Lanzador {
                exe,
                args: ["--exact", "subproceso::tests::servidor_hijo", "--nocapture", "--test-threads", "1", "-q"]
                    .map(String::from)
                    .to_vec(),
                env,
            },
            local: Some(Box::new(move || local_backend(minifier, Preset::CarrilA, kind, false))),
            hijo: Mutex::new(None),
            timeout,
            presupuesto,
        }
    }

    /// R1-H08: un minifier que no contesta a tiempo no es un panic: el mensaje
    /// lleva `PREFIJO_TIMEOUT` y `causa_recurso` lo separa (main lo manda a
    /// `recursos/` con `causa: timeout`).
    /// 9d: sin cota absoluta (antes < 10 s). El hijo duerme DORMIR antes de
    /// contestar: si el padre lo esperara (o lo matara tarde), tardaria eso. Se
    /// compara contra una base medida en el mismo test (lanzar el mismo binario de
    /// tests y minificar con un hijo que contesta enseguida), con margen DORMIR / 4:
    /// lejos de lo que cuesta esperar al hijo y de lo que mueve la carga.
    #[test]
    fn timeout_del_minifier_es_recurso() {
        const DORMIR: Duration = Duration::from_secs(20);
        let base = {
            let r = remoto_de_test_con(Minifier::Swc, SourceKind::Script, &[], Duration::from_secs(30), Some(0));
            let t0 = Instant::now();
            assert!(matches!(r.prepare("console.log(1);\n"), Outcome::Ok { .. }), "la base tenia que minificar");
            t0.elapsed()
        };
        let dormir = DORMIR.as_millis().to_string();
        let r = remoto_de_test_con(Minifier::Swc, SourceKind::Script, &[("DIFUZZ_TEST_DORMIR_MS", &dormir)], Duration::from_millis(300), Some(0));
        let t0 = Instant::now();
        let Outcome::Panic(msg) = r.prepare("console.log(1);\n") else { panic!("tenia que vencer el timeout") };
        let t = t0.elapsed();
        assert!(t < base + DORMIR / 4, "espero al hijo: {t:?} (base {base:?})");
        assert!(msg.starts_with(PREFIJO_TIMEOUT) && msg.contains("0.3 s"), "{msg}");
        assert_eq!(causa_recurso(&msg), Some("timeout"));
        // Un panic de verdad no es recurso.
        assert_eq!(causa_recurso("subproceso: el minifier murio por senal 6 (SIGABRT)"), None);
        assert_eq!(causa_recurso("byte index 1 is out of range"), None);
    }

    /// R2-H04: un hijo que pasa el presupuesto de RSS del worker se mata y el
    /// caso queda con `PREFIJO_MEMORIA` (no "senal 6"); el siguiente caso usa un
    /// hijo nuevo.
    /// 9d: sin cota absoluta (antes < 15 s). El hijo come y despues duerme
    /// DORMIR; el sondeo (cada SONDEO_MEMORIA) lo tiene que frenar mucho antes.
    /// Base y margen como en `timeout_del_minifier_es_recurso`.
    #[test]
    fn memoria_del_hijo_es_recurso_y_sigue() {
        const DORMIR: Duration = Duration::from_secs(20);
        if !std::path::Path::new("/proc/self/statm").exists() {
            return;
        }
        let base = {
            let r = remoto_de_test_con(Minifier::Swc, SourceKind::Script, &[], Duration::from_secs(30), Some(128 << 20));
            let t0 = Instant::now();
            assert!(matches!(r.prepare("console.log(1);\n"), Outcome::Ok { .. }), "la base tenia que minificar");
            t0.elapsed()
        };
        let dormir = DORMIR.as_millis().to_string();
        let r = remoto_de_test_con(Minifier::Swc, SourceKind::Script, &[("DIFUZZ_TEST_COMER_MIB", "300"), ("DIFUZZ_TEST_DORMIR_MS", &dormir)], Duration::from_secs(30), Some(128 << 20));
        let t0 = Instant::now();
        let Outcome::Panic(msg) = r.prepare("console.log(1);\n") else { panic!("tenia que pasar el presupuesto") };
        let t = t0.elapsed();
        assert!(t < base + DORMIR / 4, "el sondeo no lo freno: {t:?} (base {base:?})");
        assert!(msg.starts_with(PREFIJO_MEMORIA) && msg.contains("> 128 MiB"), "{msg}");
        assert_eq!(causa_recurso(&msg), Some("memoria"));
        assert!(HIJOS_DEL_HILO.with(|h| h.borrow().is_empty()), "el hijo muerto sigue anotado");
        // Otro Remoto del mismo hilo minifica: el hijo matado ya no cuenta.
        let r2 = remoto_de_test_con(Minifier::Swc, SourceKind::Script, &[], Duration::from_secs(30), Some(128 << 20));
        assert!(matches!(r2.prepare("console.log(1 + 2);\n"), Outcome::Ok { ref minified, .. } if minified.contains("console.log")));
    }

    /// R1-H08 / R2-H04: el hallazgo queda en `recursos/<caso>/meta.txt` con la
    /// causa explicita, y una corrida nueva sobre el mismo `--out` lo limpia.
    #[test]
    fn recurso_queda_en_recursos_con_su_causa() {
        let d = std::env::temp_dir().join(format!("difuzz-test-recursos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let rep = crate::oracle::Reporter::new(&d, "swc+carril_a+tal_cual".into(), "");
        let caso = std::path::Path::new("lento/input.js");
        let msg = format!("{PREFIJO_TIMEOUT} el minifier no termino en 60.0 s (lento o colgado; hijo matado)");
        rep.recurso(caso, "x;\n", causa_recurso(&msg).unwrap(), &msg);
        let meta = std::fs::read_to_string(d.join("recursos/lento/meta.txt")).unwrap();
        assert!(meta.contains("kind: Recursos\ncausa: timeout\n") && meta.contains(&msg), "{meta}");
        assert!(!d.join("panic").exists());
        drop(rep);
        let _rep = crate::oracle::Reporter::new(&d, "otra".into(), "");
        assert!(!d.join("recursos").exists(), "recursos/ de la corrida anterior quedo");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// R2-H04: si lo que frena es RLIMIT_AS (una reserva grande sin tocar: el
    /// RSS no sube y el sondeo no la ve), el allocator aborta y el mensaje dice
    /// memoria, no solo "senal 6".
    #[test]
    fn rlimit_as_es_memoria() {
        // presupuesto 64 MiB: RLIMIT_AS = 64 MiB + MARGEN_VIRTUAL; se piden 2 GiB.
        let r = remoto_de_test_con(Minifier::Swc, SourceKind::Script, &[("DIFUZZ_TEST_RESERVAR_MIB", "2048")], Duration::from_secs(30), Some(64 << 20));
        let Outcome::Panic(msg) = r.prepare("console.log(1);\n") else { panic!("tenia que abortar") };
        assert!(msg.starts_with(PREFIJO_MEMORIA) && msg.contains("RLIMIT_AS 1088 MiB") && msg.contains("senal 6"), "{msg}");
        assert_eq!(causa_recurso(&msg), Some("memoria"));
    }

    /// R2-H04: con `--jobs 16` la suma de los presupuestos no pasa la memoria
    /// de la maquina (ni la mitad); `--mem-hijo` manda; techo y piso.
    #[test]
    fn presupuesto_con_jobs_16_no_pasa_la_memoria() {
        let gib = 1u64 << 30;
        for total in [8 * gib, 32 * gib, 33_557_385_216, 256 * gib] {
            let (p, _) = presupuesto(Some(total), 16, None);
            assert!(16 * p <= total / 2, "total {total}: 16 x {p}");
            assert!(limite_as(p) > p);
            // b2-int: las dos partes (minificadores y analisis) suman el presupuesto.
            assert_eq!(parte_minificadores(p) + p / PARTE_ANALISIS, p);
        }
        // La maquina de verdad.
        if let Some(total) = mem_total() {
            let (p, de) = presupuesto(Some(total), 16, None);
            assert!(16 * p <= total, "{de}: 16 x {p} > {total}");
        }
        assert_eq!(presupuesto(Some(32 * gib), 1, None).0, LIMITE_MEMORIA, "techo");
        assert_eq!(presupuesto(Some(32 * gib), 16, Some(700)).0, 700 << 20, "--mem-hijo");
        let (p, aviso) = presupuesto(Some(4 * gib), 64, None);
        assert!(p == PRESUPUESTO_MINIMO && aviso.starts_with("aviso"), "{aviso}");
        assert_eq!(limite_as(0), LIMITE_MEMORIA, "sin configurar: el de siempre");
    }

    /// Aceptacion de M34: un abort real del hijo (desborde de pila) es un
    /// `Outcome::Panic` con la senal, y el caso siguiente se minifica con un
    /// hijo nuevo. Un panic comun del minifier (O6 de oxc) llega con su mensaje.
    #[test]
    fn abort_del_hijo_es_un_hallazgo_y_sigue() {
        let r = remoto_de_test(Minifier::Swc, SourceKind::Script);
        let cfg = crate::minify::Config::from_backend(Box::new(r));
        let ok = |o: Outcome| matches!(o, Outcome::Ok { ref minified, .. } if minified.contains("console.log"));
        assert!(ok(crate::minify::prepare("console.log(1 + 2);\n", &cfg)));
        let hondo = format!("var x = {}1{};\n", "[".repeat(20_000), "]".repeat(20_000));
        let Outcome::Panic(msg) = crate::minify::prepare(&hondo, &cfg) else { panic!("tenia que abortar") };
        assert!(msg.starts_with("subproceso: el minifier murio por senal 6 (SIGABRT)"), "{msg}");
        assert!(msg.contains("overflowed its stack"), "{msg}");
        // El hijo se re-lanza solo.
        assert!(ok(crate::minify::prepare("console.log(3);\n", &cfg)));
    }

    /// R1-H05: sin oraculo, `configurar_oraculo` devuelve el aviso (main lo
    /// imprime una vez) con como compilarlo, y las configs quedan sin respaldo:
    /// un panic sigue siendo panic, como antes.
    #[test]
    fn sin_oraculo_avisa_y_todo_sigue_como_antes() {
        let tmp = std::env::temp_dir().join(format!("difuzz-test-oraculo-falta-{}", std::process::id()));
        let exe = tmp.join("release").join("difuzz");
        let aviso = elegir_oraculo(&exe, None).unwrap_err();
        assert!(aviso.contains("cargo build --profile oraculo") && aviso.contains("panic/"), "{aviso}");
        assert!(aviso.contains("release/difuzz-oraculo") && aviso.contains("oraculo/difuzz"), "{aviso}");
        // DIFUZZ_ORACULO que no existe: el aviso lo nombra.
        let aviso = elegir_oraculo(&exe, Some("/no/existe/difuzz".into())).unwrap_err();
        assert!(aviso.contains("DIFUZZ_ORACULO=/no/existe/difuzz: no existe"), "{aviso}");
        // Explicito: es el unico candidato (no cae en silencio a otro).
        assert_eq!(candidatos_oraculo(&exe, Some("/x".into())), [PathBuf::from("/x")]);
        // Un ejecutable que no es un difuzz oraculo no se acepta (no el binario de
        // tests: libtest con argumentos raros podria correr la suite de nuevo).
        if std::path::Path::new("/usr/bin/true").is_file() {
            let aviso = elegir_oraculo(&exe, Some("/usr/bin/true".into())).unwrap_err();
            assert!(aviso.contains("no es el mismo difuzz"), "{aviso}");
        }
        // En el binario de tests (con debug assertions y sin hermanos) no hay oraculo.
        // (Sin debug assertions no hace falta oraculo: Ok(None).)
        assert_eq!(configurar_oraculo().is_err(), cfg!(debug_assertions));
        assert!(!hay_oraculo() && oraculo(Minifier::Oxc, Preset::CoreDefaults, SourceKind::Script, false).is_none());
        assert_eq!(candidatos_oraculo(&exe, Some("".into())).len(), 2, "DIFUZZ_ORACULO vacio no cuenta");
    }

    /// R1-H05: la identidad que valida el padre.
    #[test]
    fn info_del_oraculo() {
        assert!(info(false).starts_with(SALUDO) && info(false).ends_with("debug_assertions=off"));
        assert!(info(false).contains("oxc_minifier 0.153.0"));
        assert_ne!(info(true), info(false));
    }

    #[test]
    fn panic_del_minifier_cruza_el_pipe() {
        let r = remoto_de_test(Minifier::Oxc, SourceKind::Module);
        let cfg = crate::minify::Config::from_backend(Box::new(r));
        let Outcome::Panic(msg) = crate::minify::prepare("console.log(\"\\ud800\"[\"0\"]);\n", &cfg) else {
            panic!("O6 tenia que ser panic en oxc 0.153.0")
        };
        assert!(msg.contains("out of range") && msg.contains("\nubicacion: "), "{msg}");
    }
}
