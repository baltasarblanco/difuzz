//! A6 / R1-H12: el analisis de difuzz (no el minifier) fuera del proceso principal.
//!
//! Desde M34 la minificacion corre en un hijo (subproceso.rs), pero el analisis
//! con el parser de SWC seguia en el worker: el epilogo (`minify::instrument`),
//! el manifiesto de `--stubs`, los re-chequeos (`recheck`, `neutralize_extends`)
//! y las mutaciones. Un fuente que desbordara ese parseo (o el fixer o el
//! printer de las mutaciones) abortaba la corrida entera. MEDIDO con el binario
//! 6c0ca91: `var s = 1+1+...+1` con 1e6 sumandos, `--minifier oxc --mutaciones 1`:
//! "thread 'worker-0' has overflowed its stack", SIGABRT, sin stats.
//!
//! Ahora (salvo `--en-proceso`) esos pedidos van a un hijo de analisis por
//! worker: el mismo binario con el subcomando oculto `--analizar`, perezoso
//! (se lanza con el primer pedido), con grupo propio como el de minificar
//! (`subproceso::preparar_hijo`) y bajo el presupuesto de memoria del worker
//! (b2-int: su parte, `subproceso::presupuesto_analisis`, sondeada mientras
//! analiza y con RLIMIT_AS de respaldo; ver `esperar`). Si muere o no
//! contesta, el pedido falla, el hijo se re-lanza en el pedido siguiente y la
//! corrida sigue. Con oxc, los chequeos estaticos (`checks_on_text`) tambien
//! salen del hijo minificador y vienen aca: un abort de SWC dentro del hijo de
//! oxc salia como panic de oxc.
//!
//! R1-H12, atribucion: un fallo del analisis (panic atrapado o no, abort del
//! hijo) NO es un hallazgo del minifier. No va a `panic/`: se registra como nota
//! del caso (`registrar`; meta.txt de div/known la muestra), el worker cuenta el
//! caso en `casos_con_fallo` y el caso sigue sin ese dato (sin flags estaticos,
//! sin stubs, re-chequeo inconcluso) o se descarta con causa `analisis` (el
//! epilogo de un mudo).
//!
//! Protocolo (sin serde, en bytes, como el de subproceso.rs):
//! - el hijo saluda con `SALUDO` (lo anterior se ignora: libtest en los tests);
//! - pedido: `<op> <carril a|b> <n>\n` y n partes `<len>\n<bytes>`;
//! - respuesta: `ok <n>\n` y n partes, `nada\n` (None) o `falla <len>\n<mensaje>`.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use swc_common::{Globals, GLOBALS};

use crate::backend::{Backend, Minifier};
use crate::checks::Analisis;
use crate::minify::{Outcome, SourceKind};
use crate::mutate::Variant;

const SALUDO: &str = "difuzz-analizar v1";

/// Generoso: las mutaciones de un fixture grande reimprimen el programa varias
/// veces. Un analisis colgado se mata y el caso sigue sin ese dato.
const TIMEOUT_ANALISIS: Duration = Duration::from_secs(120);

/// R1-H12: marca en `static_flags` de que los chequeos estaticos fallaron
/// adentro del backend. Cruza el pipe del hijo minificador como un flag mas y
/// la saca `Analizado` (nunca llega a la clasificacion).
pub const FALLO_ANALISIS: &str = "fallo_analisis";

/// En el hijo minificador (`--preparar`): oxc no corre los chequeos con SWC
/// (los hace `Analizado` en el padre, via el hijo de analisis).
static CHECKS_AFUERA: AtomicBool = AtomicBool::new(false);

pub fn activar_checks_afuera() {
    CHECKS_AFUERA.store(true, Ordering::SeqCst);
}

pub fn checks_afuera() -> bool {
    CHECKS_AFUERA.load(Ordering::SeqCst)
}

thread_local! {
    /// Notas del caso en curso (una por fallo). Las limpia y cuenta el worker.
    static NOTAS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// Mensaje del ultimo fallo de los chequeos dentro de un backend en proceso.
    static ULTIMO: RefCell<Option<String>> = const { RefCell::new(None) };
    /// El hijo de analisis de este worker.
    static HIJO: RefCell<Option<Hijo>> = const { RefCell::new(None) };
    /// Tests: forzar un panic en todo analisis de este hilo.
    #[cfg(test)]
    static FORZAR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Tests: el hijo de analisis es el binario de tests (y se usa aunque
    /// `subproceso` no este activo).
    #[cfg(test)]
    static LANZADOR_TEST: RefCell<Option<Lanzador>> = const { RefCell::new(None) };
}

/// Que lanzar como hijo: ejecutable, argumentos y entorno extra.
type Lanzador = (PathBuf, Vec<String>, Vec<(String, String)>);

/// En produccion, este mismo binario con `--analizar`; en los tests, el que
/// fije `usar_hijo_de_test`.
fn lanzador() -> Lanzador {
    #[cfg(test)]
    if let Some(l) = LANZADOR_TEST.with(|l| l.borrow().clone()) {
        return l;
    }
    (std::env::current_exe().unwrap_or_else(|_| PathBuf::from("difuzz")), vec!["--analizar".to_string()], Vec::new())
}

/// Agrega una nota de fallo del analisis al caso en curso.
pub fn registrar(que: &str, msg: &str) {
    NOTAS.with(|n| n.borrow_mut().push(format!("{que}: {}", msg.trim_end())));
}

/// Las notas del caso en curso (para meta.txt).
pub fn notas() -> Vec<String> {
    NOTAS.with(|n| n.borrow().clone())
}

/// Las notas del caso en curso, y las borra (el worker, entre casos).
pub fn tomar_notas() -> Vec<String> {
    NOTAS.with(|n| std::mem::take(&mut *n.borrow_mut()))
}

fn forzar() {
    #[cfg(test)]
    if FORZAR.with(|f| f.get()) || std::env::var_os("DIFUZZ_TEST_PANIC_ANALISIS").is_some() {
        panic!("panic forzado del analisis (test)");
    }
}

/// Tests: fuerza (o no) un panic en todo analisis de este hilo.
#[cfg(test)]
pub fn forzar_panic(si: bool) {
    FORZAR.with(|f| f.set(si));
}

/// Corre `f` (analisis de difuzz) atrapando su panic. Un panic que `f` ya
/// atrapo adentro (los `catch_unwind` de instrument, manifest, checks_on_text...)
/// tambien es un fallo: se ve por la ubicacion que deja el hook (M22). Sin el
/// hook (tests) solo se ven los panics que llegan hasta aca.
pub fn atrapar<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    crate::minify::tomar_ubicacion_panic();
    let r = catch_unwind(AssertUnwindSafe(|| {
        forzar();
        f()
    }));
    let loc = crate::minify::tomar_ubicacion_panic();
    let ubic = loc.as_deref().map_or(String::new(), |l| format!("\nubicacion: {l}"));
    match r {
        Ok(v) if loc.is_none() => Ok(v),
        Ok(_) => Err(format!("panic atrapado adentro del analisis{ubic}")),
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic sin mensaje".into());
            Err(format!("panic: {msg}{ubic}"))
        }
    }
}

/// R1-H12: los chequeos estaticos adentro de un backend (swc: sobre su AST;
/// oxc en proceso: `checks_on_text`). Si fallan, sin flags ni escrituras y con
/// la marca `FALLO_ANALISIS`; el mensaje queda en `ULTIMO` (en proceso lo toma
/// `Analizado`; desde el hijo minificador no cruza el pipe).
pub fn fallo_en_backend(msg: String) {
    ULTIMO.with(|u| *u.borrow_mut() = Some(msg));
}

/// Un backend con el analisis separado del minifier. Lo arman `Preset::build`
/// y `Config::core_defaults_keep_names` alrededor del backend (o del `Remoto`).
pub struct Analizado {
    inner: Box<dyn Backend>,
    /// `Some(carril)`: los chequeos estaticos se piden aca (oxc en un hijo).
    checks: Option<SourceKind>,
}

pub fn envolver(inner: Box<dyn Backend>, minifier: Minifier, kind: SourceKind, remoto: bool) -> Box<dyn Backend> {
    let checks = (remoto && minifier == Minifier::Oxc).then_some(kind);
    Box::new(Analizado { inner, checks })
}

impl Backend for Analizado {
    fn prepare(&self, src: &str) -> Outcome {
        // Un panic del minifier sigue subiendo hasta minify::prepare_con: es hallazgo.
        let (original, minified, mut static_flags, mut dead_writes, mut analisis) = match self.inner.prepare(src) {
            Outcome::Ok { original, minified, static_flags, dead_writes, analisis } => (original, minified, static_flags, dead_writes, analisis),
            otro => return otro,
        };
        // b2-int (R1-H13): un fallo de los chequeos deja los tres textos sin
        // analizar. La marca es la unica senal que cruza el pipe del hijo
        // minificador (swc), asi que el estado `Perdido` se deriva de ella.
        if static_flags.contains(&FALLO_ANALISIS) {
            static_flags.retain(|f| *f != FALLO_ANALISIS);
            analisis = crate::checks::Analisis::PERDIDO;
            let msg = ULTIMO.with(|u| u.borrow_mut().take()).unwrap_or_else(|| {
                "panic en checks.rs adentro del hijo minificador (el mensaje no cruza el pipe; --en-proceso lo muestra)".into()
            });
            registrar("chequeos estaticos (sin flags ni lint en este programa)", &msg);
        }
        if let Some(kind) = self.checks {
            // b2-int (R1-H13): el estado del analisis viene del hijo de analisis,
            // junto con los flags (operacion `checks`).
            match checks(src, &original, &minified, kind) {
                Ok((f, d, a)) => (static_flags, dead_writes, analisis) = (f, d, a),
                Err(e) => {
                    analisis = crate::checks::Analisis::PERDIDO;
                    registrar("chequeos estaticos (sin flags ni lint en este programa)", &e)
                }
            }
        }
        Outcome::Ok { original, minified, static_flags, dead_writes, analisis }
    }
}

// ---------------------------------------------------------------------------
// Pedidos del padre.

fn carril(kind: SourceKind) -> &'static str {
    kind.carril()
}

/// En proceso o en el hijo, segun `subproceso::activo()` (tests: `LANZADOR_TEST`).
fn correr(op: &str, kind: SourceKind, partes: &[&str]) -> Result<Option<Vec<String>>, String> {
    if !en_hijo() {
        return atrapar(|| ejecutar(op, kind, partes));
    }
    HIJO.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            match lanzar() {
                Ok(h) => *slot = Some(h),
                Err(e) => {
                    eprintln!("analisis: no pude lanzar el hijo de analisis: {e}; analizo en proceso");
                    drop(slot);
                    return atrapar(|| ejecutar(op, kind, partes));
                }
            }
        }
        let h = slot.as_mut().expect("recien lanzado");
        let enviado = enviar(&mut h.stdin, op, kind, partes);
        let r = match enviado {
            Ok(()) => match esperar(h) {
                Ok(r) => r,
                // b2-int (R2-H04): paso su parte del presupuesto del worker. Es
                // un fallo del analisis (nota), nunca un hallazgo del minifier.
                Err((rss, limite)) => {
                    return Err(slot.take().expect("hijo").muerte(&format!(
                        "paso su presupuesto de memoria: {} MiB de RSS > {} MiB; hijo matado",
                        rss >> 20,
                        limite >> 20
                    )))
                }
            },
            Err(e) => Ok(Err(e)),
        };
        match r {
            Ok(Ok(Resp::Ok(v))) => Ok(v),
            Ok(Ok(Resp::Falla(m))) => Err(m),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::InvalidData => {
                Err(slot.take().expect("hijo").muerte(&format!("error de protocolo ({e}); hijo matado")))
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => Err(slot.take().expect("hijo").muerte("")),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(slot
                .take()
                .expect("hijo")
                .muerte(&format!("no termino en {} s; hijo matado", TIMEOUT_ANALISIS.as_secs()))),
        }
    })
}

/// b2-int (R2-H04): espera la respuesta hasta `TIMEOUT_ANALISIS`, mirando cada
/// `SONDEO_MEMORIA` el RSS del hijo contra su parte del presupuesto del worker
/// (`subproceso::presupuesto_analisis`; 0: sin sondeo). Si se pasa,
/// `Err((rss, presupuesto))`.
#[allow(clippy::type_complexity)]
fn esperar(h: &Hijo) -> Result<Result<std::io::Result<Resp>, mpsc::RecvTimeoutError>, (u64, u64)> {
    let presupuesto = crate::subproceso::presupuesto_analisis();
    let t0 = std::time::Instant::now();
    loop {
        let falta = TIMEOUT_ANALISIS.saturating_sub(t0.elapsed());
        if falta.is_zero() {
            return Ok(Err(mpsc::RecvTimeoutError::Timeout));
        }
        let tramo = if presupuesto == 0 { falta } else { falta.min(crate::subproceso::SONDEO_MEMORIA) };
        match h.rx.recv_timeout(tramo) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            r => return Ok(r),
        }
        if presupuesto > 0 {
            let rss = crate::subproceso::rss(h.child.id());
            if rss > presupuesto {
                return Err((rss, presupuesto));
            }
        }
    }
}

fn en_hijo() -> bool {
    #[cfg(test)]
    if LANZADOR_TEST.with(|l| l.borrow().is_some()) {
        return true;
    }
    crate::subproceso::activo()
}

fn enviar(w: &mut impl Write, op: &str, kind: SourceKind, partes: &[&str]) -> std::io::Result<()> {
    writeln!(w, "{op} {} {}", carril(kind), partes.len())?;
    for p in partes {
        writeln!(w, "{}", p.len())?;
        w.write_all(p.as_bytes())?;
    }
    w.flush()
}

/// El epilogo observador (`minify::instrument`).
pub fn epilogo(src: &str, invocar: bool, kind: SourceKind) -> Result<Option<String>, String> {
    correr("epilogo", kind, &[src, if invocar { "1" } else { "0" }]).map(|v| v.and_then(|mut v| v.pop()))
}

/// El manifiesto de `--stubs`. Si el analisis falla: `{}` (sin stubs) y una nota.
pub fn stubs(src: &str, kind: SourceKind) -> String {
    match correr("stubs", kind, &[src]) {
        Ok(Some(mut v)) if v.len() == 1 => v.pop().unwrap_or_default(),
        Ok(_) => "{}".into(),
        Err(e) => {
            registrar("manifiesto de --stubs (sin stubs en este programa)", &e);
            "{}".into()
        }
    }
}

/// Los globals conocidos que usa el programa (recheck::globals_matter).
pub fn globals(src: &str, kind: SourceKind) -> Result<Option<Vec<String>>, String> {
    correr("globals", kind, &[src])
}

/// `recheck::neutralize_with`.
pub fn sin_with(src: &str, kind: SourceKind) -> Result<Option<String>, String> {
    correr("with", kind, &[src]).map(|v| v.and_then(|mut v| v.pop()))
}

/// `backend::swc::neutralize_extends`.
pub fn sin_extends(src: &str, kind: SourceKind) -> Result<Option<String>, String> {
    correr("extends", kind, &[src]).map(|v| v.and_then(|mut v| v.pop()))
}

/// `backend::swc::checks_on_text`. b2-int (R1-H13): la cuarta parte es el
/// estado del analisis de los tres textos (`checks::Analisis::codigo`).
#[allow(clippy::type_complexity)]
pub fn checks(src: &str, original: &str, minified: &str, kind: SourceKind) -> Result<(Vec<&'static str>, (usize, usize), Analisis), String> {
    let v = correr("checks", kind, &[src, original, minified])?.ok_or("checks sin respuesta")?;
    let [f, d0, d1, a] = &v[..] else {
        return Err(format!("checks: {} partes", v.len()));
    };
    let num = |s: &str| s.parse::<usize>().map_err(|_| format!("checks: numero {s:?}"));
    let flags = if f.is_empty() { Vec::new() } else { f.split(',').map(crate::subproceso::intern_flag).collect() };
    let analisis = Analisis::de_codigo(a).ok_or_else(|| format!("checks: estado del analisis {a:?}"))?;
    Ok((flags, (num(d0)?, num(d1)?), analisis))
}

/// b2-int (R1-H14): el epilogo de la pasada de bloques (`minify::instrument_bloques`).
pub fn bloques(src: &str, kind: SourceKind) -> Result<Option<String>, String> {
    correr("bloques", kind, &[src]).map(|v| v.and_then(|mut v| v.pop()))
}

/// `mutate::variants_ext`.
pub fn variantes(src: &str, n: usize, extra: usize, seed: u64, kind: SourceKind) -> Result<Option<Vec<Variant>>, String> {
    let (n_s, e_s, s_s) = (n.to_string(), extra.to_string(), seed.to_string());
    let Some(v) = correr("mutar", kind, &[src, &n_s, &e_s, &s_s])? else {
        return Ok(None);
    };
    if v.len() % 4 != 0 {
        return Err(format!("mutar: {} partes", v.len()));
    }
    Ok(Some(
        v.chunks(4)
            .map(|c| Variant { src: c[0].clone(), desc: c[1].clone(), extra: c[2] == "1", tdz_filter: c[3] == "1" })
            .collect(),
    ))
}

/// Lo que hace el analisis, en el proceso que sea. `None`: el analisis no da
/// nada (no parsea, no hay que cambiar).
fn ejecutar(op: &str, kind: SourceKind, p: &[&str]) -> Option<Vec<String>> {
    let b = |x: bool| if x { "1" } else { "0" }.to_string();
    match (op, p) {
        ("epilogo", [src, inv]) => crate::minify::instrument(src, *inv == "1", kind).map(|s| vec![s]),
        ("stubs", [src]) => Some(vec![crate::stubs::manifest(src, kind)]),
        ("globals", [src]) => GLOBALS.set(&Globals::new(), || {
            crate::backend::swc::parse_for(src, kind).map(|p| crate::checks::known_globals_used(&p))
        }),
        ("with", [src]) => crate::recheck::neutralize_with(src, kind).map(|s| vec![s]),
        ("extends", [src]) => crate::backend::swc::neutralize_extends(src, kind).map(|s| vec![s]),
        ("checks", [src, o, m]) => {
            let (f, (d0, d1), a) = crate::backend::swc::checks_on_text(src, o, m, kind);
            Some(vec![f.join(","), d0.to_string(), d1.to_string(), a.codigo()])
        }
        ("bloques", [src]) => crate::minify::instrument_bloques(src, kind).map(|s| vec![s]),
        ("mutar", [src, n, e, s]) => {
            let num = |x: &str| x.parse::<u64>().unwrap_or(0);
            let vs = crate::mutate::variants_ext(src, num(n) as usize, num(e) as usize, num(s), kind)?;
            Some(
                vs.into_iter()
                    .flat_map(|v| [v.src, v.desc, b(v.extra), b(v.tdz_filter)])
                    .collect(),
            )
        }
        _ => panic!("analisis: pedido desconocido {op} con {} partes", p.len()),
    }
}

// ---------------------------------------------------------------------------
// El hijo (`difuzz --analizar`).

enum Resp {
    Ok(Option<Vec<String>>),
    Falla(String),
}

struct Hijo {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<std::io::Result<Resp>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_hilo: Option<std::thread::JoinHandle<()>>,
}

fn lanzar() -> std::io::Result<Hijo> {
    let (exe, args, env) = lanzador();
    let mut cmd = Command::new(&exe);
    cmd.args(&args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // b2-int (R2-H04): RLIMIT_AS de respaldo sobre su parte del presupuesto.
    crate::subproceso::preparar_hijo(&mut cmd, crate::subproceso::limite_as(crate::subproceso::presupuesto_analisis()));
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().expect("stdin pedido como piped");
    let stdout = child.stdout.take().expect("stdout pedido como piped");
    let stderr_pipe = child.stderr.take().expect("stderr pedido como piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || leer_respuestas(BufReader::new(stdout), tx));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let buf = Arc::clone(&stderr);
    let stderr_hilo = std::thread::spawn(move || crate::subproceso::juntar_cola(stderr_pipe, buf));
    Ok(Hijo { child, stdin, rx, stderr, stderr_hilo: Some(stderr_hilo) })
}

impl Hijo {
    /// Mata (si sigue vivo), cosecha y arma el mensaje del fallo.
    fn muerte(mut self, causa: &str) -> String {
        let _ = self.child.kill();
        let st = self.child.wait();
        if let Some(t) = self.stderr_hilo.take() {
            let _ = t.join();
        }
        let cola = String::from_utf8_lossy(&self.stderr.lock().unwrap_or_else(|e| e.into_inner())).into_owned();
        let mut msg = if causa.is_empty() {
            match st {
                Ok(st) => crate::subproceso::describir(st).replacen("subproceso: el minifier", "el hijo de analisis", 1),
                Err(e) => format!("el hijo de analisis murio y no pude cosecharlo: {e}"),
            }
        } else {
            format!("el hijo de analisis {causa}")
        };
        if !cola.trim().is_empty() {
            msg.push_str("\nstderr del hijo de analisis:\n");
            msg.push_str(cola.trim_end());
        }
        msg
    }
}

impl Drop for Hijo {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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

fn leer_linea(r: &mut impl BufRead) -> std::io::Result<String> {
    let mut l = String::new();
    if r.read_line(&mut l)? == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok(l.trim_end_matches('\n').to_string())
}

fn leer_partes(r: &mut impl BufRead, n: usize) -> std::io::Result<Vec<String>> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let l = leer_linea(r)?;
        let len = l.parse::<usize>().map_err(|_| invalido(format!("largo: {l:?}")))?;
        v.push(leer_bytes(r, len)?);
    }
    Ok(v)
}

fn leer_resp(r: &mut impl BufRead) -> std::io::Result<Resp> {
    let cab = leer_linea(r)?;
    let campos: Vec<&str> = cab.split(' ').collect();
    let num = |s: &str| s.parse::<usize>().map_err(|_| invalido(format!("cabecera: {cab:?}")));
    match campos[..] {
        ["ok", n] => Ok(Resp::Ok(Some(leer_partes(r, num(n)?)?))),
        ["nada"] => Ok(Resp::Ok(None)),
        ["falla", n] => Ok(Resp::Falla(leer_bytes(r, num(n)?)?)),
        _ => Err(invalido(format!("cabecera: {cab:?}"))),
    }
}

fn escribir_resp(w: &mut impl Write, r: &Result<Option<Vec<String>>, String>) -> std::io::Result<()> {
    match r {
        Ok(Some(v)) => {
            writeln!(w, "ok {}", v.len())?;
            for p in v {
                writeln!(w, "{}", p.len())?;
                w.write_all(p.as_bytes())?;
            }
            Ok(())
        }
        Ok(None) => w.write_all(b"nada\n"),
        Err(m) => {
            writeln!(w, "falla {}", m.len())?;
            w.write_all(m.as_bytes())
        }
    }
}

fn leer_respuestas(mut r: impl BufRead, tx: mpsc::Sender<std::io::Result<Resp>>) {
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
        let o = leer_resp(&mut r);
        let fin = o.is_err();
        if tx.send(o).is_err() || fin {
            return;
        }
    }
}

/// El subcomando oculto `difuzz --analizar`. Devuelve el exit code.
pub fn serve() -> i32 {
    std::panic::set_hook(Box::new(crate::minify::record_panic_location));
    let salida = std::io::BufWriter::new(std::io::stdout());
    serve_con_pila(crate::WORKER_STACK, BufReader::new(std::io::stdin()), salida)
}

fn serve_con_pila(pila: usize, entrada: impl BufRead + Send, salida: impl Write + Send) -> i32 {
    // Misma pila que un worker (M1): lo que pasaba en proceso pasa aca.
    let r = std::thread::scope(|s| {
        std::thread::Builder::new()
            .name("analizar".into())
            .stack_size(pila)
            .spawn_scoped(s, move || bucle(entrada, salida))
            .map(|h| h.join())
    });
    match r {
        Ok(Ok(Ok(()))) => 0,
        Ok(Ok(Err(e))) => {
            eprintln!("--analizar: {e}");
            1
        }
        _ => 1,
    }
}

fn bucle(mut entrada: impl BufRead, mut salida: impl Write) -> std::io::Result<()> {
    writeln!(salida, "{SALUDO}")?;
    salida.flush()?;
    loop {
        let cab = match leer_linea(&mut entrada) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let campos: Vec<&str> = cab.split(' ').collect();
        let [op, k, n] = campos[..] else {
            return Err(invalido(format!("pedido: {cab:?}")));
        };
        let kind = SourceKind::parse(k).ok_or_else(|| invalido(format!("carril: {cab:?}")))?;
        let n = n.parse::<usize>().map_err(|_| invalido(format!("pedido: {cab:?}")))?;
        let partes = leer_partes(&mut entrada, n)?;
        let refs: Vec<&str> = partes.iter().map(String::as_str).collect();
        let r = atrapar(|| ejecutar(op, kind, &refs));
        escribir_resp(&mut salida, &r)?;
        salida.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El hijo de los tests: el binario de tests filtrado a este test. Sin la
    /// variable no hace nada (pasa). Pila de 1 MiB: un anidamiento moderado la
    /// desborda y el hijo aborta de verdad (SIGABRT).
    #[test]
    fn servidor_analisis() {
        if std::env::var_os("DIFUZZ_TEST_ANALISIS").is_none() {
            return;
        }
        std::panic::set_hook(Box::new(crate::minify::record_panic_location));
        let salida = std::io::BufWriter::new(std::io::stdout());
        std::process::exit(serve_con_pila(1 << 20, BufReader::new(std::io::stdin()), salida));
    }

    /// Este hilo usa como hijo de analisis el binario de tests (con `env` extra).
    pub(crate) fn usar_hijo_de_test(env: &[(&str, &str)]) {
        let exe = std::env::current_exe().unwrap();
        let args = ["--exact", "analisis::tests::servidor_analisis", "--nocapture", "--test-threads", "1", "-q"]
            .map(String::from)
            .to_vec();
        let mut e: Vec<(String, String)> = vec![("DIFUZZ_TEST_ANALISIS".into(), "1".into())];
        e.extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        LANZADOR_TEST.with(|l| *l.borrow_mut() = Some((exe, args, e)));
        HIJO.with(|h| h.borrow_mut().take());
    }

    pub(crate) fn sin_hijo_de_test() {
        LANZADOR_TEST.with(|l| l.borrow_mut().take());
        HIJO.with(|h| h.borrow_mut().take());
    }

    #[test]
    fn protocolo_ida_y_vuelta() {
        for r in [Ok(Some(vec!["a\nb ñ".to_string(), String::new()])), Ok(None), Err("x\ny".to_string())] {
            let mut v = Vec::new();
            escribir_resp(&mut v, &r).unwrap();
            match (leer_resp(&mut &v[..]).unwrap(), r) {
                (Resp::Ok(a), Ok(b)) => assert_eq!(a, b),
                (Resp::Falla(a), Err(b)) => assert_eq!(a, b),
                _ => panic!("distinto"),
            }
        }
        assert!(leer_resp(&mut &b"ok 2\n1\na"[..]).is_err());
    }

    /// En proceso y en el hijo, los pedidos dan lo mismo que llamar directo.
    #[test]
    fn el_hijo_contesta_igual_que_en_proceso() {
        let src = "var o = { valueOf() { return 1 } };\nwith (o) { x; }\nclass A extends 1 {}\n[1][0] = 2;\nconsole.log(typeof window);\n";
        let k = SourceKind::Script;
        let directo = (
            crate::minify::instrument(src, true, k),
            crate::recheck::neutralize_with(src, k),
            crate::backend::swc::neutralize_extends(src, k),
            crate::backend::swc::checks_on_text(src, src, src, k),
            crate::mutate::variants_ext(src, 3, 2, 7, k).unwrap().into_iter().map(|v| (v.src, v.desc, v.extra, v.tdz_filter)).collect::<Vec<_>>(),
        );
        let pedir = || {
            (
                epilogo(src, true, k).unwrap(),
                sin_with(src, k).unwrap(),
                sin_extends(src, k).unwrap(),
                checks(src, src, src, k).unwrap(),
                variantes(src, 3, 2, 7, k).unwrap().unwrap().into_iter().map(|v| (v.src, v.desc, v.extra, v.tdz_filter)).collect::<Vec<_>>(),
            )
        };
        assert_eq!(pedir(), directo, "en proceso");
        assert_eq!(globals(src, k).unwrap(), Some(vec!["window".to_string()]));
        assert_eq!(stubs("import { a } from 'x';\n", SourceKind::Module), "{\"x\":[\"a\"]}");
        usar_hijo_de_test(&[]);
        assert_eq!(pedir(), directo, "en el hijo");
        assert_eq!(globals(src, k).unwrap(), Some(vec!["window".to_string()]));
        assert_eq!(stubs("import { a } from 'x';\n", SourceKind::Module), "{\"x\":[\"a\"]}");
        assert_eq!(epilogo("let = ;", false, k).unwrap(), None);
        // b2-int: la pasada de bloques (R1-H14) y un estado del analisis no
        // limpio (R1-H13) cruzan el pipe del hijo de analisis tal cual.
        let bl = "var a = 1;\nif (a) { var b = 2; }\n";
        assert_eq!(bloques(bl, k).unwrap(), crate::minify::instrument_bloques(bl, k));
        assert!(bloques(bl, k).unwrap().is_some_and(|e| e.ends_with("__difuzzObserve([\"b\", b]);\n")));
        assert_eq!(bloques("var a = 1;\n", k).unwrap(), None);
        let rec = "new.target;\n";
        let (_, _, a) = checks(rec, rec, "x = {", k).unwrap();
        assert_eq!(a.codigo(), "rrp");
        assert_eq!(checks(rec, rec, "x = {", k).unwrap(), crate::backend::swc::checks_on_text(rec, rec, "x = {", k));
        sin_hijo_de_test();
        assert!(tomar_notas().is_empty());
    }

    /// A6, criterio 1: un analisis que desborda la pila aborta el hijo de
    /// analisis (SIGABRT real), no al proceso; el pedido falla con la senal y el
    /// siguiente se atiende con un hijo nuevo.
    #[test]
    fn abort_del_analisis_no_tumba_el_proceso() {
        usar_hijo_de_test(&[]);
        let hondo = format!("var x = {}1{};\n", "[".repeat(20_000), "]".repeat(20_000));
        let e = epilogo(&hondo, false, SourceKind::Script).unwrap_err();
        assert!(e.starts_with("el hijo de analisis murio por senal 6 (SIGABRT)"), "{e}");
        assert!(e.contains("overflowed its stack"), "{e}");
        let m = stubs(&format!("import {{ a }} from 'x';\n{hondo}"), SourceKind::Module);
        assert_eq!(m, "{}");
        let notas = tomar_notas();
        assert!(notas.len() == 1 && notas[0].starts_with("manifiesto de --stubs") && notas[0].contains("SIGABRT"), "{notas:?}");
        // b2-int (R1-H14): la pasada de bloques tambien parsea en el hijo: su
        // abort es un fallo del pedido, no del proceso.
        let e = bloques(&format!("if (1) {{ {hondo} }}\n"), SourceKind::Script).unwrap_err();
        assert!(e.starts_with("el hijo de analisis murio por senal 6 (SIGABRT)"), "{e}");
        // El hijo se re-lanza solo.
        assert_eq!(epilogo("var y = 1;\n", false, SourceKind::Script).unwrap().unwrap(), "var y = 1;\n\n;\n__difuzzObserve([\"y\", y]);\n");
        sin_hijo_de_test();
    }

    /// R1-H12, criterio 2: un panic forzado de los chequeos estaticos adentro
    /// del backend (swc: sobre su AST; oxc: `checks_on_text`) no es un panic del
    /// minifier: el caso se minifica, sin flags ni escrituras, con una nota.
    #[test]
    fn panic_de_los_chequeos_no_es_panic_del_minifier() {
        use crate::minify::{prepare, Preset};
        let src = "var o = { valueOf() { return 1 } };\n[1, 2][0] = 3;\nconsole.log(+o);\n";
        for m in [Minifier::Swc, Minifier::Oxc] {
            let cfg = Preset::CarrilA.build(m, SourceKind::Script);
            tomar_notas();
            forzar_panic(true);
            let o = prepare(src, &cfg);
            forzar_panic(false);
            let Outcome::Ok { static_flags, dead_writes, minified, .. } = o else { panic!("{m:?}: tenia que minificar") };
            assert!(static_flags.is_empty() && dead_writes == (0, 0) && minified.contains("console.log"), "{m:?}");
            let notas = tomar_notas();
            assert!(notas.len() == 1 && notas[0].starts_with("chequeos estaticos") && notas[0].contains("panic forzado"), "{m:?}: {notas:?}");
            // Control: sin forzar, los flags de siempre y sin notas.
            let Outcome::Ok { static_flags, dead_writes, .. } = prepare(src, &cfg) else { panic!() };
            assert_eq!((static_flags, dead_writes.0), (vec!["coercion"], 1), "{m:?}");
            assert!(tomar_notas().is_empty());
        }
        // oxc con los chequeos afuera (como con el hijo minificador): los pide
        // `Analizado` al hijo de analisis; si ahi fallan, tambien es nota.
        let inner = Minifier::Oxc.build(Preset::CarrilA, SourceKind::Script);
        let b = envolver(inner, Minifier::Oxc, SourceKind::Script, true);
        usar_hijo_de_test(&[("DIFUZZ_TEST_PANIC_ANALISIS", "1")]);
        let Outcome::Ok { minified, analisis, .. } = b.prepare(src) else { panic!("oxc tenia que minificar") };
        sin_hijo_de_test();
        assert!(minified.contains("console.log"));
        // b2-int (R1-H13): sin analisis, los tres textos quedan perdidos.
        assert_eq!(analisis, Analisis::PERDIDO);
        let notas = tomar_notas();
        assert!(notas.iter().any(|n| n.contains("panic forzado") && n.contains("ubicacion: src/analisis.rs")), "{notas:?}");
        // b2-int (R1-H13): oxc con los chequeos afuera: el estado del analisis
        // lo trae la operacion `checks` del hijo de analisis (no el pipe del
        // hijo minificador). `new.target` top-level: SWC lo recupera.
        let inner = Minifier::Oxc.build(Preset::CarrilA, SourceKind::Script);
        let b = envolver(inner, Minifier::Oxc, SourceKind::Script, true);
        usar_hijo_de_test(&[]);
        let Outcome::Ok { analisis, .. } = b.prepare("console.log(new.target);\n") else { panic!("oxc tenia que minificar") };
        sin_hijo_de_test();
        assert_eq!(analisis.fuente, crate::checks::Parseo::Recuperado, "{analisis:?}");
        assert!(tomar_notas().is_empty());
    }

    /// R1-H12: un panic del analisis (tambien uno que la funcion atrapa adentro)
    /// es un fallo del analisis, en proceso y en el hijo.
    #[test]
    fn panic_del_analisis_es_fallo_del_analisis() {
        forzar_panic(true);
        let e = epilogo("var y = 1;\n", false, SourceKind::Script).unwrap_err();
        assert!(e.starts_with("panic: panic forzado del analisis (test)"), "{e}");
        forzar_panic(false);
        usar_hijo_de_test(&[("DIFUZZ_TEST_PANIC_ANALISIS", "1")]);
        let e = checks("x;", "x;", "x;", SourceKind::Script).unwrap_err();
        assert!(e.starts_with("panic: panic forzado del analisis (test)") && e.contains("\nubicacion: src/analisis.rs:"), "{e}");
        sin_hijo_de_test();
        // Un panic atrapado adentro de `f` se ve por la ubicacion del hook.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(crate::minify::record_panic_location));
        let r = atrapar(|| catch_unwind(|| panic!("adentro")).is_err());
        std::panic::set_hook(prev);
        assert!(r.unwrap_err().starts_with("panic atrapado adentro del analisis\nubicacion: src/analisis.rs:"));
    }
}
