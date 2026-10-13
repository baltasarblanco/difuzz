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

/// I3 (revision-b456): lineas del canal de control cuando el sensor de
/// violaciones no corrio entero (`Exec::sensor_degradado`).
pub const SENSOR_CAIDO: &str = "sensor_caido";
pub const SENSOR_TOPE: &str = "sensor_tope";
/// b7-8: el caso corrio entero sin el sensor porque su original tiene el patron del bug
/// de V8 (`Runner::sin_sensor_por_riesgo`, `checks::riesgo_sensor_v8`).
pub const SENSOR_OMITIDO: &str = "sensor_omitido";

/// 7.4: los flags que el preload pone por cada clase de violacion que ubico con una linea
/// `viol` sin marca de no atrapada (`FLAG_VIOL` en preload.js). Los de una violacion
/// atrapada solo los ve el sensor; `coercion_symbol` tambien sale de un Symbol no atrapado.
pub const FLAGS_DEL_SENSOR: [&str; 3] = ["tdz_atrapada", "coercion_symbol", "extends_atrapado"];

/// Integracion 7.1 + 7.4 (H3 de rev-7.1): el tope de pausas del sensor (`DIFUZZ_MAX_PAUSAS`,
/// preload.js) de la corrida de B con el sensor que da su camino cuando A y B se compararon sin
/// el (`recheck::camino_sensor_i3`). 0,5-1,4 ms por pausa (MEDIDO por 7.4): 2-6 s; con muchos
/// marcos puede llegar al timeout (rev-int H2), y entonces el camino no se sabe.
pub const MAX_PAUSAS_MAYOR: &str = "4096";

/// n-sandbox: linea del canal de control cuando la ejecucion toco el sandbox (un permiso
/// denegado: la escribe el preload, `notarSandbox`, o `run_file` si la salida lo muestra).
pub const SANDBOX: &str = "sandbox";

/// 8.2: linea del canal de control cuando el throw no atrapado que termino el programa es un
/// permiso negado (la escribe el preload en `onThrow`, junto con `sandbox`). La usa
/// `Runner::murio_por_sandbox`.
pub const SANDBOX_SIN_ATRAPAR: &str = "sandbox_sin_atrapar";

/// n-sandbox (`--sandbox`, `--sandbox-red`): con que encierro corre node. A, B,
/// confirmaciones, re-chequeos y `--invocar` pasan todos por `run_file` con el mismo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Sandbox {
    /// Node sin restricciones (lo de siempre).
    #[default]
    Apagado,
    /// Modelo de permisos de node 24 (`--permission`, ver `permisos_node`) y senales solo
    /// al propio node (`encerrar_senales`).
    Permisos,
    /// Permisos mas espacios de nombres de Linux sin root (usuario, red y montajes, ver
    /// `Aislar`): sin red (solo un loopback propio), /run tapado y los sockets unix del
    /// sistema tapados con /dev/null. Solo Linux.
    PermisosYRed,
}

impl Sandbox {
    pub fn activo(self) -> bool {
        self != Sandbox::Apagado
    }

    pub fn nombre(self) -> &'static str {
        match self {
            Sandbox::Apagado => "apagado",
            Sandbox::Permisos => "permisos",
            Sandbox::PermisosYRed => "permisos+red",
        }
    }
}

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

/// 11.5 (I3): tope de RSS de una ejecucion de node con un runner sin configurar (`Runner::new`;
/// los tests). La corrida lo fija con `Runner::con_tope_rss` (`subproceso::tope_node`, `--mem-node`).
/// Es el techo de los hijos minificadores (`subproceso::LIMITE_MEMORIA`).
pub const TOPE_RSS_SIN_CONFIGURAR: u64 = 4 << 30;

/// 11.5 (I3): cada cuanto se mira el RSS de node mientras corre (el bucle de espera de
/// `run_file` ya despierta cada 5 ms; el RSS se lee cada 2 vueltas). Lo que un programa reserva y
/// toca entre dos lecturas pasa el tope antes de que lo maten (MEDIDO, ver el INFORME de 11b); el
/// pico (`ru_maxrss`) igual lo ve al cosecharlo.
const SONDEO_RSS: u32 = 2;

/// 11.5 (I3): causa de `recursos/` (meta.txt) y comienzo del mensaje de una ejecucion de node que
/// paso el tope de RSS.
pub const NODE_MEMORIA: &str = "node_memoria";

/// 11.7 (I5): tope por defecto de la salida que difuzz junta de una ejecucion de node (stdout mas
/// stderr), en MiB (`--max-salida`). MEDIDO: la entrada mas grande de las 77 caches de A de las
/// corridas del 7 al 12 de octubre (254 442 entradas: sintetico, corpus SWC y corpus real de D2)
/// pesa 1,4 MiB con el programa incluido. difuzz copia la salida unas 5 veces (decodificar,
/// normalizar, comparar; MEDIDO en I5: 512 MiB de stdout, 2,6 GB de RSS): con 32 workers, 16 MiB
/// son ~2,5 GB en el peor caso.
pub const MAX_SALIDA_MIB: u64 = 16;

/// 11.7 (I5): causa de `recursos/` de una ejecucion que escribio mas que el tope de salida.
pub const NODE_SALIDA: &str = "node_salida";

/// 11.5 (I3): una ejecucion de node paso un tope de difuzz (memoria). No es un resultado del
/// programa ni un error del runner: lo que se leyo de ella no se compara. `run_file` no devuelve
/// un `Exec`: desenrolla con esto (`std::panic::resume_unwind`, que no llama al hook de panic)
/// hasta quien procesa el caso (`main`: el worker, y cada variante de `--mutaciones`), que lo manda
/// a `recursos/` con su causa. Asi ningun re-chequeo, confirmacion ni pasada observada puede
/// tomar una ejecucion cortada por un tope como si hubiera terminado (regla madre: ni iguales, ni
/// divergencia, ni known/, ni descartes por algo de difuzz).
/// 11.7 (I5): tambien el tope de la salida que se junta (`NODE_SALIDA`): una salida cortada no se
/// compara como si estuviera entera.
#[derive(Debug)]
pub struct Excedido {
    /// `NODE_MEMORIA` o `NODE_SALIDA`.
    pub causa: &'static str,
    /// Que tope, cuanto y que archivo (para meta.txt).
    pub mensaje: String,
    /// El programa que corrio node (el texto del caso, del original o del minificado, o de un
    /// re-chequeo), para repetirlo a mano.
    pub programa: String,
    /// Extension del programa (`cjs` o `mjs`).
    pub ext: &'static str,
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

    /// I3 (revision-b456): esta ejecucion no corrio entera con el sensor de
    /// violaciones: `SENSOR_CAIDO` (se repitio sin el tras una senal o un
    /// timeout, `run_inner`) o `SENSOR_TOPE` (el preload llego a MAX_PAUSAS y lo
    /// apago a mitad). `None`: entera con el sensor, o sin el por politica.
    /// b7-8: tambien `SENSOR_OMITIDO` (sin el por el patron del bug de V8): nunca
    /// cuenta como entera con el sensor.
    pub fn sensor_degradado(&self) -> Option<&'static str> {
        [SENSOR_CAIDO, SENSOR_TOPE, SENSOR_OMITIDO].into_iter().find(|s| self.ctl.lines().any(|l| l == *s))
    }

    /// 7.4 (Fase R I3): `self` es el mismo programa repetido sin el sensor porque el sensor
    /// se degrado (`compare_and_report`), y `con` su corrida con el sensor (entera, o a
    /// medias: `SENSOR_TOPE`). Le suma lo que el sensor ubico en `con`: las lineas `viol`
    /// que `self` no tiene y los `FLAGS_DEL_SENSOR`. Devuelve las lineas `viol` agregadas,
    /// sin el prefijo. Solo le dice al re-chequeo de violaciones donde neutralizar: la
    /// comparacion y los re-chequeos siguen sin el sensor (simetricos), y lo que manda a
    /// known/ es su veredicto (`veredicto_neutralizado`: B == A', todo sin el sensor), no
    /// estas lineas. Las violaciones posteriores al tope no se vieron: no hay que heredar.
    pub fn heredar_violaciones(&mut self, con: &Exec) -> Vec<String> {
        let mut nuevas: Vec<&str> = Vec::new();
        for l in con.ctl.lines().filter(|l| l.starts_with("viol ")) {
            if !nuevas.contains(&l) && !self.ctl.lines().any(|p| p == l) {
                nuevas.push(l);
            }
        }
        for l in &nuevas {
            if !self.ctl.is_empty() && !self.ctl.ends_with('\n') {
                self.ctl.push('\n');
            }
            self.ctl.push_str(l);
            self.ctl.push('\n');
        }
        for f in FLAGS_DEL_SENSOR {
            if con.flags.iter().any(|x| x == f) && !self.flags.iter().any(|x| x == f) {
                self.flags.push(f.to_string());
            }
        }
        // Como los deja el preload (`flags` ordenados).
        self.flags.sort();
        nuevas.iter().map(|l| l["viol ".len()..].to_string()).collect()
    }

    /// n-sandbox: esta ejecucion toco el sandbox (un permiso denegado, atrapado o no). El
    /// preload solo la escribe con `--permission` y `run_file` solo mira la salida con el
    /// sandbox prendido; main pregunta por `Runner::toco_sandbox`, que exige el sandbox.
    pub fn toco_sandbox(&self) -> bool {
        self.ctl.lines().any(|l| l == SANDBOX)
    }

    /// b7-1: la cobertura de bloques que dejo el preload (`cobertura <hash> <rangos>`),
    /// solo si corrio con `Runner::run_con_cobertura`. La ultima linea, si hay varias.
    pub fn cobertura(&self) -> Option<&str> {
        self.ctl.lines().rev().find_map(|l| l.strip_prefix("cobertura "))
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

    /// 11.2 (I2 de rev-integral-1): el codigo con el que el programa eligio terminar
    /// (`process.exit(k)`, `process.exitCode = k`) si no es 0, sin lanzar y sin ser el fallo
    /// de enlace del preload (`link_failed`). `None`: exit 0, un throw (THROW_EXIT con su
    /// linea en el canal), timeout, senal o falla del runner.
    pub fn codigo_propio(&self) -> Option<i32> {
        match self.status {
            Status::Exited(k) if k != 0 && !self.threw() && !self.link_failed() => Some(k),
            _ => None,
        }
    }

    /// 11.2: termino por si mismo y hay algo que comparar: `completed` o un codigo propio.
    /// El original que sale con un codigo propio se compara (estado + stdout, `oracle::compare`)
    /// en vez de descartarse como `exit`. `completed` sigue siendo exit 0 o throw: los
    /// re-chequeos lo piden a sus variantes del original y una variante que sale con un codigo
    /// propio los deja inconclusos (div/), como antes.
    fn termino_solo(&self) -> bool {
        self.completed() || self.codigo_propio().is_some()
    }

    /// Algo que comparar: el programa imprimio, o lanzo (y el tipo esta en stdout).
    /// 11.2: tambien con un codigo de salida propio (`codigo_propio`).
    pub fn ok_with_output(&self) -> bool {
        self.termino_solo() && (self.threw() || self.own_output())
    }

    pub fn ok_silent(&self) -> bool {
        self.termino_solo() && !self.threw() && !self.own_output()
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

/// R1-H22: cuenta los `Runner` vivos (no los observadores) que usan un
/// directorio. El ultimo en soltarse lo borra si quedo vacio. Sin esto, cada
/// `cargo test` dejaba un `run_dir()` vacio: los tests usan preloads fuera de
/// el (`write_tagged`) y el Drop de `Preload` no lo tocaba. En una corrida el
/// preload esta adentro, el directorio no queda vacio y lo borra el Drop de
/// `Preload`, como antes. Crear y borrar van bajo el mismo lock: mientras un
/// runner vive, su directorio existe.
struct Inquilinos(Mutex<usize>);

impl Inquilinos {
    const fn new() -> Inquilinos {
        Inquilinos(Mutex::new(0))
    }

    fn entrar(&self, dir: &Path) {
        let mut n = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *n += 1;
        // Si falla, el primer `run` lo reporta como Failed (no se puede escribir).
        let _ = std::fs::create_dir_all(dir);
    }

    fn salir(&self, dir: &Path) {
        let mut n = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        if *n == 0 {
            // remove_dir, no remove_dir_all: si queda algo ajeno, no se toca.
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// R1-H22: los inquilinos de `run_dir()`.
static INQUILINOS: Inquilinos = Inquilinos::new();

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
    /// Decision del usuario (09-oct): el sensor de violaciones del preload (b4-a)
    /// solo corre si la politica del backend lo usa (`Politica::usa_sensor`).
    /// `false`: toda ejecucion lleva DIFUZZ_SENSOR=0. Por defecto `true`.
    sensor: bool,
    /// n-sandbox: el encierro de node (`--sandbox`). Por defecto apagado.
    sandbox: Sandbox,
    /// b7-8: sin el sensor por el patron del bug de V8 (`sin_sensor_por_riesgo`).
    omitido: bool,
    /// 8.7: con el sensor prendido desde el arranque aunque `puede_atrapar` diga que no
    /// (`sensor_entero`). No prende nada si `sensor` es false (ni con `omitido`).
    entero: bool,
    /// 11.5 (I3): tope de RSS de cada ejecucion de node, en bytes (0: sin tope). Ver `Excedido`.
    tope_rss: u64,
    /// 11.7 (I5): tope de la salida (stdout + stderr) que se junta de cada ejecucion, en bytes (0:
    /// sin tope). Ver `Excedido`.
    tope_salida: u64,
    /// Copia que comparte los archivos del runner original (`observing`,
    /// `sin_sensor`): al soltarse no borra nada.
    prestado: bool,
}

impl Runner {
    pub fn new(preload: &Preload, worker: usize, timeout: Duration) -> Runner {
        let dir = run_dir();
        // R1-H22: crea el directorio y lo cuenta (lo borra el ultimo Drop).
        INQUILINOS.entrar(&dir);
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
            sensor: true,
            sandbox: Sandbox::Apagado,
            omitido: false,
            entero: false,
            tope_rss: TOPE_RSS_SIN_CONFIGURAR,
            tope_salida: MAX_SALIDA_MIB << 20,
            prestado: false,
        }
    }

    /// 11.7 (I5): el tope de salida (bytes, stdout + stderr) de cada ejecucion de este runner y de
    /// sus copias; 0, sin tope. Al pasarlo, node muere y `run_file` desenrolla con `Excedido`.
    pub fn con_tope_salida(mut self, bytes: u64) -> Runner {
        self.tope_salida = bytes;
        self
    }

    /// 11.5 (I3): el tope de RSS (bytes) de cada ejecucion de este runner y de sus copias; 0, sin
    /// tope. Al pasarlo, node muere y `run_file` desenrolla con `Excedido`.
    pub fn con_tope_rss(mut self, bytes: u64) -> Runner {
        self.tope_rss = bytes;
        self
    }

    /// n-sandbox: todas las ejecuciones de este runner (y de sus copias) con `s`.
    pub fn with_sandbox(mut self, s: Sandbox) -> Runner {
        self.sandbox = s;
        self
    }

    /// n-sandbox: `e` (corrida por este runner) toco el sandbox. Sin sandbox, false siempre:
    /// ni una linea `sandbox` que un programa escriba a mano en el canal cambia nada.
    pub fn toco_sandbox(&self, e: &Exec) -> bool {
        self.sandbox.activo() && e.toco_sandbox()
    }

    /// 8.2: `e` (corrida por este runner) termino por un throw no atrapado que es un permiso
    /// negado del sandbox: lo que seguia a ese punto no corrio. El preload lo anota
    /// (`SANDBOX_SIN_ATRAPAR`: el objeto lanzado tiene el `code` de un permiso negado, aunque el
    /// stack no lo diga, como ERR_DLOPEN_DISABLED) o el mensaje y el stack del throw (la linea
    /// `exc`) tienen uno de los textos de `marca_sandbox` (con `--sandbox-red`, tambien un error
    /// de red; `node:sqlite` apagado). Mira solo el throw que mato al programa, no un permiso
    /// negado y atrapado antes. Sin sandbox, false siempre.
    pub fn murio_por_sandbox(&self, e: &Exec) -> bool {
        self.toco_sandbox(e)
            && e.threw()
            && (e.ctl.lines().any(|l| l == SANDBOX_SIN_ATRAPAR) || e.exc().is_some_and(|x| marca_sandbox(&x, self.sandbox)))
    }

    /// Decision del usuario (09-oct): con `false`, node corre siempre sin el
    /// sensor de violaciones (A, B y re-chequeos: todo pasa por este runner).
    pub fn with_sensor(mut self, on: bool) -> Runner {
        self.sensor = on;
        self
    }

    /// Si este runner corre node con el sensor de violaciones.
    pub fn con_sensor(&self) -> bool {
        self.sensor
    }

    /// I3 (revision-b456): el mismo runner (mismos archivos) sin el sensor, para
    /// repetir A y B sin el cuando uno de los dos corrio sin el (o a medias).
    pub fn sin_sensor(&self) -> Runner {
        let mut r = self.prestar();
        r.sensor = false;
        r
    }

    /// b7-8: el mismo runner sin el sensor para TODO un caso cuyo original tiene el
    /// patron del bug de V8 (`checks::riesgo_sensor_v8`): A, B, confirmacion,
    /// re-chequeos y pasadas observadas corren igual, sin el. Cada ejecucion lleva
    /// `sensor_omitido` en el canal y la cache de A la guarda aparte (`cache_tag`).
    pub fn sin_sensor_por_riesgo(&self) -> Runner {
        let mut r = self.sin_sensor();
        r.omitido = true;
        r
    }

    /// b7-8: este runner corre sin el sensor por el patron del bug de V8.
    pub fn sensor_omitido(&self) -> bool {
        self.omitido
    }

    /// 8.7: el mismo runner con el sensor prendido desde el arranque, sin el pre-filtro
    /// perezoso (`puede_atrapar`). Para el camino de B (`recheck::camino_de_b`): un falso
    /// negativo del pre-filtro sobre el minificado (nombres armados en runtime) dejaba a B sin
    /// las lineas `viol` de lo que atrapa. Sin el sensor (`sin_sensor`, `omitido`) no cambia nada.
    pub fn sensor_entero(&self) -> Runner {
        let mut r = self.prestar();
        r.entero = true;
        r
    }

    /// Copia que usa los mismos archivos y no los borra al soltarse.
    fn prestar(&self) -> Runner {
        Runner {
            script: self.script.clone(),
            preload: self.preload.clone(),
            timeout: self.timeout,
            kind: self.kind,
            wrapper: self.wrapper.clone(),
            observe: self.observe,
            invocar: self.invocar,
            ctl: self.ctl.clone(),
            cwd: self.cwd.clone(),
            stubs: self.stubs.clone(),
            sensor: self.sensor,
            sandbox: self.sandbox,
            omitido: self.omitido,
            entero: self.entero,
            tope_rss: self.tope_rss,
            tope_salida: self.tope_salida,
            prestado: true,
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

    /// b7-1b: script o modulo (lo que corre este runner), para instrumentar el minificado.
    pub fn kind(&self) -> SourceKind {
        self.kind
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
        let mut r = self.prestar();
        r.observe = true;
        r
    }

    pub fn run(&self, js: &str) -> Exec {
        self.run_inner(js, Opts::default())
    }

    /// M28: lo que distingue la ejecucion de `run` en la cache de A ademas del
    /// texto: el envoltorio observador (y si invoca) corre otro archivo.
    /// b7-8: y el sensor omitido (si no, un A con el sensor de un binario anterior,
    /// mismo preload, se compararia contra un B sin el).
    pub fn cache_tag(&self) -> &'static str {
        match (self.observe, self.invocar, self.omitido) {
            (false, _, false) => "",
            (true, false, false) => "observar",
            (true, true, false) => "observar+invocar",
            (false, _, true) => SENSOR_OMITIDO,
            (true, false, true) => "observar sensor_omitido",
            (true, true, true) => "observar+invocar sensor_omitido",
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

    /// t-principio: igual que `run`, con variables de entorno extra para el preload
    /// (re-chequeos de contenido fijo, DIFUZZ_SALT=fijo, y de built-ins,
    /// DIFUZZ_BUILTINS_FIJOS).
    pub fn run_with_env(&self, js: &str, env: &[(&str, &str)]) -> Exec {
        self.run_inner(js, Opts { env, ..Opts::default() })
    }

    /// b7-1: igual que `run`, con argumentos extra para node, variables de entorno extra
    /// para el preload y la cobertura de bloques de V8 (DIFUZZ_COBERTURA=1,
    /// `Exec::cobertura`). Con el sandbox prendido, el inspector se permite aunque no
    /// haya sensor: la cobertura lo usa.
    pub fn run_con_cobertura(&self, js: &str, node_args: &[&str], env: &[(&str, &str)]) -> Exec {
        self.run_inner(js, Opts { node_args, env, cobertura: true, ..Opts::default() })
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

    /// b4-a: el sensor de violaciones atrapadas (inspector de V8 en el preload)
    /// puede tumbar a node o volverlo lentisimo (MEDIDO: con pausa en excepciones
    /// atrapadas cada throw recorre la pila; una recursion que atrapa en cada nivel
    /// pasa de 0,05 s a mas de 40 s). La caida es un bug de V8 13.6 (node 24.14.1), no
    /// del preload: una pausa dentro de la definicion de una clase en codigo sloppy con
    /// `eval` directo arma los scopes con el contexto equivocado y termina en SIGSEGV
    /// en el GC (MEDIDO con fixture/issues/9460/side-effects y con un repro sin difuzz:
    /// un preload que abre una `inspector.Session`, `Debugger.enable`,
    /// `Debugger.setPauseOnExceptions({ state: 'caught' })` y `Debugger.resume` en cada pausa,
    /// y el programa sloppy de 3 lineas
    /// `var v; try { class X extends 1 {} } catch {} if (false) eval("");`: SIGSEGV 10/10,
    /// 0/10 sin el preload, sin el `eval` o con "use strict"). b7-8: los casos con
    /// ese patron ya no prenden el sensor (`sin_sensor_por_riesgo`); esto queda para
    /// lo que el patron no ve (codigo armado en runtime, otro bug). Si node muere por
    /// una senal o no termina a tiempo con el sensor, se repite sin el: el resultado
    /// es el de siempre (sin flags de violacion) y el canal lleva `sensor_caido`.
    ///
    /// Paso 7 (revision-tint I1): ante Timeout la repeticion sin el sensor usa el
    /// timeout entero, como en b4c98fd. La sonda de M2 (un cuarto del timeout, minimo
    /// 1 s) perdia un div/ real (corpus-sonda/t01: una recursion que atrapa, lenta con
    /// el sensor, y despues un bucle de ~2,5 s y el bug; sin el sensor tarda mas que la
    /// sonda y quedaba en Timeout). El costo: un programa que cuelga de verdad (y que
    /// tiene algo que el sensor ve) paga dos timeouts enteros en cada corrida (A, B, la
    /// confirmacion de B y cada re-chequeo). No se puede saber antes si la demora es
    /// del sensor: en la recursion que atrapa en el limite de la pila, la primera pausa
    /// tarda ~50 s en llegar al handler del sensor (MEDIDO, node 24.14.1, 6307 frames).
    ///
    /// t-sensor: si el programa no puede atrapar una excepcion (`puede_atrapar`),
    /// el sensor va perezoso (lo prende `invocar` del preload, si llega). El
    /// reintento ante senal o timeout queda igual que con el sensor prendido.
    fn run_inner(&self, js: &str, opts: Opts) -> Exec {
        if self.omitido {
            let mut e = self.run_once(js, Opts { sin_sensor: true, ..opts });
            e.ctl.insert_str(0, &format!("{SENSOR_OMITIDO}\n"));
            return e;
        }
        let opts = Opts { sin_sensor: opts.sin_sensor || !self.sensor, ..opts };
        let opts = Opts { perezoso: !opts.sin_sensor && !self.entero && !puede_atrapar(js), ..opts };
        let e = self.run_once(js, opts);
        if !matches!(e.status, Status::Signal | Status::Timeout) || opts.sin_sensor {
            return e;
        }
        let mut e = self.run_once(js, Opts { sin_sensor: true, ..opts });
        e.ctl.insert_str(0, &format!("{SENSOR_CAIDO}\n"));
        e
    }

    fn run_once(&self, js: &str, opts: Opts) -> Exec {
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
        if self.prestado {
            return;
        }
        let _ = std::fs::remove_file(&self.script);
        let _ = std::fs::remove_file(&self.wrapper);
        let _ = std::fs::remove_file(&self.ctl);
        let _ = std::fs::remove_dir_all(&self.cwd);
        if let Some(p) = &self.stubs {
            let _ = std::fs::remove_file(p);
        }
        // R1-H22: el directorio de este runner es el de `script` (Runner::new).
        if let Some(dir) = self.script.parent() {
            INQUILINOS.salir(dir);
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
    /// b4-a: sin el sensor de violaciones del preload (DIFUZZ_SENSOR=0).
    sin_sensor: bool,
    /// t-principio: variables de entorno extra (`run_with_env`).
    env: &'a [(&'a str, &'a str)],
    /// t-sensor: el sensor no se prende al arrancar (DIFUZZ_SENSOR=perezoso), ver
    /// `puede_atrapar`. Lo pone `run_inner`; `sin_sensor` manda sobre esto.
    perezoso: bool,
    /// b7-1: con la cobertura de bloques del preload (`run_con_cobertura`).
    cobertura: bool,
}

/// t-sensor: si el programa puede atrapar una excepcion, directa o indirectamente,
/// y por eso necesita el sensor de violaciones desde el arranque. Prender el sensor
/// cuesta un `Debugger.enable` por node (MEDIDO, node 24: ~17 ms de pared en un
/// programa trivial, contra ~27 ms del node entero con el preload).
///
/// El sensor solo pausa en excepciones que V8 predice atrapadas. Sin nada de esta
/// lista en el texto, el programa no tiene quien las atrape, salvo node y el preload:
/// - node: los timers, nextTick, el cargador CJS, los callbacks de fs y `emit` no
///   pausan (MEDIDO, TDZ dentro de cada uno). Si pausan (MEDIDO) el top-level de un
///   modulo ES y `queueMicrotask`, pero el error termina sin atrapar: `onThrow` saca la
///   violacion y lo unico que cambia es `stack`, que la pausa prendia al formatear.
/// - el preload: `invocar` atrapa lo que lanzan las funciones del programa; por eso
///   el modo perezoso lo prende ahi. Paso 7 (revision-tint I2): tambien lo prenden
///   `onThrow` (si lo lanzado es un objeto que no es un Error nativo comun, o si hay un
///   built-in pisado segun la foto del preload) y las entradas de la observacion antes de
///   `ser` (si hay un built-in pisado), porque s03/s04 llegaban a codigo propio sin nada de
///   la lista (un @@iterator o @@toPrimitive alcanzado con getPrototypeOf/getOwnPropertySymbols).
///   Lo demas del preload que atrapa (prepareStackTrace, `ser` sin built-ins pisados)
///   llega a codigo del programa solo por lo que esta en la lista (Proxy, getters de
///   `stack`/`message`, toString/valueOf, prototipos, subclases e introspeccion).
///
/// Sobre-aproximado a proposito: el texto entero, strings y comentarios incluidos,
/// sin parsear. Con `\u` (un identificador escapado) o ante la duda: true.
/// Riesgo conocido (no cubierto): codigo que arma estos nombres en runtime
/// (`globalThis["Pro" + "mise"]`) o que pisa globals que el preload usa al observar
/// (`Object`, `JSON`, `Array`) con funciones propias.
pub fn puede_atrapar(js: &str) -> bool {
    const PALABRAS: &[&str] = &[
        // Atrapan en el programa: try/catch/finally, promesas y thenables, async.
        "try", "catch", "finally", "async", "await", "then", "fromAsync", "waitAsync",
        "queueMicrotask", "FinalizationRegistry",
        // Codigo que no esta en el texto, o que node corre adentro de un try.
        "eval", "Function", "require", "inspect", "stdout", "stderr",
        "EventTarget", "addEventListener", "dispatchEvent", "AbortController", "AbortSignal",
        // Codigo del programa que el preload puede llamar adentro de un try.
        "Proxy", "Symbol", "Reflect", "extends", "prototype", "__proto__", "setPrototypeOf",
        "create", "defineProperty", "defineProperties", "__defineGetter__", "__defineSetter__",
        "stack", "message", "toString", "valueOf", "prepareStackTrace", "captureStackTrace",
        // Paso 7 (revision-tint I2, s03/s04): introspeccion que llega a prototipos que la
        // foto de built-ins del preload no mira (los de los iteradores) y a los Symbol bien
        // conocidos sin escribir `Symbol` (un @@iterator propio en una instancia de Map).
        "getPrototypeOf", "getOwnPropertySymbols", "getOwnPropertyDescriptor", "getOwnPropertyDescriptors",
    ];
    if js.contains("\\u") || js.contains("romis") {
        return true;
    }
    let b = js.as_bytes();
    let es_id = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    // El primer byte que no es espacio desde `i` hacia adelante (o hacia atras con
    // `atras`). Un byte no ASCII cuenta como algo que no es espacio: ante la duda, true.
    let sig = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        (i < b.len()).then(|| (i, b[i]))
    };
    let atras = |mut i: usize| {
        while i > 0 && b[i - 1].is_ascii_whitespace() {
            i -= 1;
        }
        (i > 0).then(|| b[i - 1])
    };
    let mut i = 0;
    while i < b.len() {
        if !es_id(b[i]) {
            i += 1;
            continue;
        }
        let ini = i;
        while i < b.len() && es_id(b[i]) {
            i += 1;
        }
        let w = &js[ini..i];
        if PALABRAS.contains(&w) {
            return true;
        }
        let despues = sig(i);
        match w {
            // `import(...)`, el import dinamico (el estatico no atrapa nada).
            "import" if matches!(despues, Some((_, b'(' | b'/' | 0x80..=0xff))) => return true,
            // Un metodo `return` propio: IteratorClose descarta lo que lanza cuando el
            // cuerpo del for-of ya lanzo (lo atrapa V8). `x.return`, `return: f`,
            // `"return"` y `return(...) {`.
            "return" => {
                let q = ini.checked_sub(1).map(|j| b[j]);
                if matches!(atras(ini), Some(b'.' | 0x80..=0xff))
                    || matches!(despues, Some((_, b':' | 0x80..=0xff)))
                    || (matches!(q, Some(b'"' | b'\'' | b'`')) && b.get(i) == q.as_ref())
                {
                    return true;
                }
                if let Some((p, b'(')) = despues {
                    // El `)` que cierra (con parametros que llaman: `return(a = f()) {`) y
                    // tambien el primero (sobra, pero asi el filtro solo agrega casos a la
                    // version que se audito sobre el corpus de SWC). Sin cierre: true.
                    let primero = b[p..].iter().position(|&c| c == b')');
                    let mut prof = 0usize;
                    let cierre = b[p..].iter().position(|&c| {
                        match c {
                            b'(' => prof += 1,
                            b')' => prof -= 1,
                            _ => {}
                        }
                        prof == 0
                    });
                    for c in [primero, cierre] {
                        match c {
                            Some(c) if !matches!(sig(p + c + 1), Some((_, b'{'))) => {}
                            _ => return true,
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
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
    // 11.6 (I4): node no sobrevive a difuzz (`comando_node`). Con el sandbox ya hay pre_exec (fork):
    // el prctl va ahi, despues del encierro.
    let (mut cmd, prctl_en_pre_exec) = comando_node(if r.sandbox.activo() { None } else { setpriv() });
    cmd.current_dir(&r.cwd).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("TMPDIR", &tmpdir).env("HOME", &r.cwd);
    // --no-warnings: los avisos de node (p. ej. "Invalid asm.js") traen el PID y
    // no son salida del programa.
    cmd.arg("--no-warnings").args(opts.node_args);
    // n-sandbox: A y B (y todo lo demas) con las mismas flags; el sensor necesita el inspector.
    if r.sandbox.activo() {
        cmd.args(permisos_node(r, !opts.sin_sensor || opts.cobertura));
    }
    if r.sandbox.activo() {
        let aislar = (r.sandbox == Sandbox::PermisosYRed).then(Aislar::armar);
        // SAFETY: `aplicar` y `encerrar_senales` corren entre fork y exec y solo hacen
        // llamadas al sistema sobre memoria armada antes o en la pila (no reservan, no
        // toman locks).
        unsafe {
            cmd.pre_exec(move || encerrar(aislar.as_ref()));
        }
    }
    if prctl_en_pre_exec {
        // 11.6 (I4): despues del sandbox (que cambia credenciales), el ultimo paso antes del exec.
        prctl_en_el_hijo(&mut cmd);
    }
    cmd.arg("--require").arg(preload).arg(script).env("TZ", "UTC");
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
    if opts.sin_sensor {
        cmd.env("DIFUZZ_SENSOR", "0");
    } else if opts.perezoso {
        cmd.env("DIFUZZ_SENSOR", "perezoso");
    }
    if opts.cobertura {
        cmd.env("DIFUZZ_COBERTURA", "1");
    }
    for (k, v) in opts.env {
        cmd.env(k, v);
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
    // 11.7 (I5): lo que se junta de los dos pipes, contra el tope de salida.
    let salida = Arc::new(Salida { tope: r.tope_salida, ..Salida::default() });
    let out_buf = drain(child.stdout.take().expect("stdout fue pedido como piped"), tx.clone(), Arc::clone(&salida));
    let err_buf = drain(child.stderr.take().expect("stderr fue pedido como piped"), tx, Arc::clone(&salida));

    // 11.5 (I3): node se espera con `wait4` (no `Child::try_wait`) para tener su pico de RSS
    // (`ru_maxrss`) al cosecharlo; desde aca `child` no se espera ni se mata por std. Mientras
    // corre, su RSS se lee cada `SONDEO_RSS` vueltas: si pasa `tope_rss`, se mata el grupo.
    let mut cos = Cosecha::new(pid);
    // 11.6 (I4): mientras corre, su grupo lo puede matar la segunda senal (`matar_vivos`).
    let vivo = Vivo::registrar(pid);
    let mut rss_visto = 0u64;
    let mut matado_por_rss = false;
    let mut vuelta = 0u32;
    let status = loop {
        match cos.esperar(false) {
            Ok(true) => break cos.status(),
            Ok(false) => {
                // 11.7 (I5): paso el tope de salida: lo que siga no se junta; se corta aca.
                if salida.excedida.load(std::sync::atomic::Ordering::SeqCst) {
                    cos.matar();
                    break Status::Signal;
                }
                vuelta = vuelta.wrapping_add(1);
                if r.tope_rss > 0 && vuelta.is_multiple_of(SONDEO_RSS) {
                    rss_visto = rss_visto.max(crate::subproceso::rss(pid));
                    if rss_visto > r.tope_rss {
                        matado_por_rss = true;
                        cos.matar();
                        break Status::Signal;
                    }
                }
                if start.elapsed() >= timeout {
                    cos.matar();
                    break Status::Timeout;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                cos.matar();
                break Status::Failed(format!("wait: {e}"));
            }
        }
    };
    drop(vivo);
    let pico = cos.pico_rss().max(rss_visto);
    let por_memoria = r.tope_rss > 0 && (matado_por_rss || pico > r.tope_rss);
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
    // 11.5 (I3) / 11.7 (I5): lo que se leyo de una ejecucion que paso un tope no se compara (ver
    // `Excedido`). La salida primero: lo juntado ya esta cortado (y node pudo terminar solo antes de
    // que el bucle lo viera: igual se cuenta, lo que se escribio paso el tope).
    if salida.excedida.load(std::sync::atomic::Ordering::SeqCst) {
        let n = salida.usado.load(std::sync::atomic::Ordering::SeqCst);
        exceder(r, script, NODE_SALIDA, format!("escribio por stdout y stderr {} MiB o mas: paso el tope de salida de {} MiB (lo de mas no se junto; matada al verlo)", n >> 20, r.tope_salida >> 20));
    }
    if por_memoria {
        let como = if matado_por_rss { "matada al pasarlo" } else { "el pico cayo entre dos lecturas; termino sola" };
        exceder(r, script, NODE_MEMORIA, format!("paso el tope de RSS de node: {} MiB > {} MiB ({como})", pico >> 20, r.tope_rss >> 20));
    }

    let mut ctl = std::fs::read(&r.ctl).map(|b| decode_lossless(&b)).unwrap_or_default();
    let (stdout, stderr) = (decode_lossless(&stdout), decode_lossless(&stderr));
    // n-sandbox: lo que el preload no ve (un permiso denegado que el programa atrapo e
    // imprimio, el mensaje del throw en la linea `exc`) se reconoce en la salida.
    if r.sandbox.activo() && [&stdout, &stderr, &ctl].iter().any(|s| marca_sandbox(s, r.sandbox)) {
        ctl.push_str(SANDBOX);
        ctl.push('\n');
    }
    Exec {
        status,
        stdout,
        stderr,
        flags: ctl_flags(&ctl),
        elapsed: start.elapsed(),
        ctl,
    }
}

/// n-sandbox: textos que solo aparecen si el sandbox nego algo (MEDIDO, node 24.14.1):
/// el codigo y el mensaje de ERR_ACCESS_DENIED y el de un addon nativo; con la red
/// cortada, los errores de conexion y de DNS (afuera da ENETUNREACH, MEDIDO; un nombre que
/// no esta en /etc/hosts no resuelve y un servicio del localhost de la maquina no esta en
/// el loopback propio). Sobre-aproximado: un programa que imprime uno de estos textos por
/// su cuenta (o un ECONNREFUSED/ENOTFOUND que sin el sandbox tambien habria dado) tambien
/// cuenta: el caso se descarta, nunca se reporta de mas.
fn marca_sandbox(s: &str, modo: Sandbox) -> bool {
    // `kill EPERM`: una senal a otro proceso que el filtro de `encerrar_senales` nego (MEDIDO:
    // node imprime `Error: kill EPERM`); sin el sandbox solo la da un proceso de otro usuario.
    // 7.7: `os.setPriority` a otro proceso, negado por el mismo filtro (MEDIDO node 24.14.1:
    // `A system error occurred: uv_os_setpriority returned EPERM (operation not permitted)`;
    // subir la prioridad propia sin privilegios da EACCES, no EPERM).
    // 7.10: `node:sqlite` apagado (`--no-experimental-sqlite`, ver `permisos_node`): `require` e
    // `import` dan ERR_UNKNOWN_BUILTIN_MODULE con este mensaje (MEDIDO node 24.14.1); sin el
    // sandbox el modulo existe y el mensaje no sale. El codigo solo no alcanza: tambien lo da
    // un `node:` que no existe.
    const PERMISOS: &[&str] = &[
        "ERR_ACCESS_DENIED",
        "Access to this API has been restricted",
        "ERR_DLOPEN_DISABLED",
        "kill EPERM",
        "uv_os_setpriority returned EPERM",
        "No such built-in module: node:sqlite",
    ];
    const RED: &[&str] = &["ENETUNREACH", "EHOSTUNREACH", "EAI_AGAIN", "ENOTFOUND", "ECONNREFUSED"];
    PERMISOS.iter().any(|m| s.contains(m)) || (modo == Sandbox::PermisosYRed && RED.iter().any(|m| s.contains(m)))
}

/// n-sandbox: las flags del modelo de permisos (`node --help`, v24.14.1: `--permission`,
/// `--allow-fs-read`, `--allow-fs-write`, `--allow-inspector`; ademas existen
/// `--allow-child-process`, `--allow-worker`, `--allow-addons` y `--allow-wasi`, que no se
/// pasan: sin ellas no hay procesos hijos, workers, addons ni WASI). Lectura: el preload, el
/// programa, el envoltorio de modulos (que lo importa), el manifiesto de stubs y el
/// directorio de trabajo del caso; escritura: el directorio de trabajo y el canal de
/// control (el preload lo abre al arrancar). Node no necesito leer nada mas para arrancar,
/// cargar un `.cjs`/`.mjs` ni resolver un modulo que no existe (MEDIDO: MODULE_NOT_FOUND,
/// no ERR_ACCESS_DENIED). 8.4: resolver un DIRECTORIO que existe (`require('..')`) si lee su
/// `package.json` y node lo niega (ERR_ACCESS_DENIED, MEDIDO node 24.14.1); no se permite: el
/// preload toma ese package.json negado como uno que no existe (`Module._findPath` envuelto) y
/// node da MODULE_NOT_FOUND, como sin el sandbox. `--allow-inspector` solo con el sensor de
/// violaciones (sin el, el sensor no se prende: MEDIDO, sin lineas `viol`).
///
/// 7.10: `--no-experimental-sqlite` (`node --help`, v24.14.1). `node:sqlite` no pasa por el
/// modelo de permisos: con `--permission` solo, `new DatabaseSync(ruta)` creaba y escribia
/// una base fuera de lo permitido sin ERR_ACCESS_DENIED (MEDIDO, b7-9 y b7-10). Con la flag,
/// `require`/`import` de `node:sqlite` dan ERR_UNKNOWN_BUILTIN_MODULE y no se crea nada
/// (MEDIDO); `marca_sandbox` lo reconoce. Web Storage (`localStorage`, que con
/// `--localstorage-file` persiste en disco) no necesita flag: en 24.14.1 viene apagado
/// (MEDIDO: `typeof localStorage` es `undefined`) y `--no-experimental-webstorage` no figura
/// en `node --help`.
fn permisos_node(r: &Runner, sensor: bool) -> Vec<std::ffi::OsString> {
    let flag = |f: &str, p: &Path| {
        let mut s = std::ffi::OsString::from(f);
        s.push(p);
        s
    };
    let mut v = vec![std::ffi::OsString::from("--permission"), "--no-experimental-sqlite".into()];
    for p in [&r.preload, &r.script, &r.wrapper, &r.cwd] {
        v.push(flag("--allow-fs-read=", p));
    }
    if let Some(s) = &r.stubs {
        v.push(flag("--allow-fs-read=", s));
    }
    v.push(flag("--allow-fs-write=", &r.cwd));
    v.push(flag("--allow-fs-write=", &r.ctl));
    if sensor {
        v.push("--allow-inspector".into());
        // b7-1b (revision 7.1, H5): con el inspector permitido, el programa podria abrir un
        // puerto con `process._debugProcess` o SIGUSR1 (MEDIDO, node 24.14.1); esto los corta
        // (MEDIDO). `inspector.open` lo niega el preload.
        v.push("--disable-sigusr1".into());
    }
    v
}

/// n-sandbox (`--sandbox-red`): lo que el hijo necesita para aislarse, armado en el padre
/// (entre fork y exec no se puede reservar memoria). Sin root, con user namespaces
/// (MEDIDO en esta maquina: `unshare --user --map-current-user --net` anda).
///
/// Por que no alcanza con la red: el modelo de permisos de node 24 no frena `connect` a
/// un socket unix (MEDIDO: un node con `--permission` leyo de un socket fuera de lo
/// permitido, tambien dentro de un namespace de red), y por los sockets de /run se llega
/// al DNS de systemd-resolved (MEDIDO: `getent hosts _gateway` resolvio dentro del
/// namespace de red), a D-Bus y a otros servicios. Por eso, ademas de la red: /run tapado
/// con un tmpfs y cada socket unix del sistema fuera de /run tapado con /dev/null.
struct Aislar {
    uid_map: Vec<u8>,
    gid_map: Vec<u8>,
    tapar: Vec<std::ffi::CString>,
}

impl Aislar {
    fn armar() -> Aislar {
        // SAFETY: getuid y getgid no fallan ni tocan memoria.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        Aislar {
            uid_map: format!("{uid} {uid} 1").into_bytes(),
            gid_map: format!("{gid} {gid} 1").into_bytes(),
            tapar: sockets_del_sistema(),
        }
    }

    /// En el hijo, entre fork y exec: namespaces de usuario (el mismo uid y gid adentro),
    /// montajes y red; montajes privados; tmpfs en /run; /dev/null sobre cada socket; y el
    /// loopback del namespace prendido (un programa que se conecta a si mismo por 127.0.0.1
    /// anda igual que sin el sandbox; lo de afuera da ENETUNREACH). Al hacer exec con un
    /// uid que no es 0, node pierde las capacidades del namespace: no puede desmontar nada
    /// (y sin `--allow-child-process` ni addons tampoco tiene con que).
    fn aplicar(&self) -> std::io::Result<()> {
        let err = std::io::Error::last_os_error;
        let nulo = std::ptr::null::<libc::c_char>();
        // SAFETY: llamadas al sistema con punteros a datos vivos (self, literales).
        unsafe {
            if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS | libc::CLONE_NEWNET) != 0 {
                return Err(err());
            }
            escribir(c"/proc/self/setgroups", b"deny")?;
            escribir(c"/proc/self/uid_map", &self.uid_map)?;
            escribir(c"/proc/self/gid_map", &self.gid_map)?;
            if libc::mount(nulo, c"/".as_ptr(), nulo, libc::MS_REC | libc::MS_PRIVATE, std::ptr::null()) != 0 {
                return Err(err());
            }
            let flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
            if libc::mount(c"tmpfs".as_ptr(), c"/run".as_ptr(), c"tmpfs".as_ptr(), flags, std::ptr::null()) != 0 {
                return Err(err());
            }
            for s in &self.tapar {
                if libc::mount(c"/dev/null".as_ptr(), s.as_ptr(), nulo, libc::MS_BIND, std::ptr::null()) != 0 {
                    // Un socket que desaparecio entre la lista y el fork ya no hace falta taparlo.
                    let e = err();
                    if e.raw_os_error() != Some(libc::ENOENT) {
                        return Err(e);
                    }
                }
            }
            prender_loopback()
        }
    }
}

/// n-sandbox: lo que corre en el hijo entre fork y exec con `--sandbox` o `--sandbox-red`:
/// los namespaces (solo con la red) y, al final, el filtro de senales.
fn encerrar(aislar: Option<&Aislar>) -> std::io::Result<()> {
    if let Some(a) = aislar {
        a.aplicar()?;
    }
    // SAFETY: solo llamadas al sistema con punteros a la pila.
    unsafe { encerrar_senales() }
}

/// 2b (revision-n2): el modelo de permisos de node no cubre las senales y no hay namespace
/// de PID: `process.kill(<pid de afuera>, ...)` llegaba a cualquier proceso del usuario
/// (MEDIDO por el revisor con un senuelo y SIGTERM). Un filtro seccomp (sin root: con
/// `no_new_privs`) deja mandar senales solo al propio node: `kill` a su pid, a 0 o a su
/// grupo (`process_group(0)`: pgid = pid de node, y sin `--allow-child-process` no tiene
/// hijos); `tgkill`, `tkill`, `rt_sigqueueinfo` y `rt_tgsigqueueinfo` a su pid (la
/// `raise`/`pthread_kill` de glibc y V8 usan `tgkill` con el tgid propio). El resto da
/// EPERM, y `pidfd_send_signal`, siempre.
///
/// 7.7 (revision fe0cd86, IMPORTANTE 1): `os.setPriority(<pid de afuera>, 19)` le bajaba la
/// prioridad a cualquier proceso del usuario, y sin `CAP_SYS_NICE` no se la puede devolver
/// (MEDIDO por el revisor con un senuelo). `setpriority` con `PRIO_PROCESS` o `PRIO_PGRP`
/// solo a 0 o al propio node (su pid; su grupo, por `process_group(0)`); `PRIO_USER` (todos
/// los procesos del usuario) y cualquier otro `which`, EPERM. De las demas llamadas que
/// tocan otro proceso por su pid (`sched_setaffinity`, `sched_set*`, `ioprio_set`,
/// `prlimit64`, `process_vm_*`, `ptrace`, `kcmp`, `pidfd_open`, `pidfd_getfd`), node 24 sin
/// procesos hijos, workers ni addons no tiene ninguna API que las llame con un pid elegido
/// por el programa (MEDIDO con strace: las APIs de node con un pid son `process.kill`,
/// `process._kill`, `process._debugProcess`, que llaman a `kill`, y `os.setPriority` y
/// `os.getPriority`, que solo lee; node llama a `prlimit64` y `sched_getaffinity` solo
/// sobre si mismo): no se filtran.
///
/// Otra arquitectura en la llamada (`int 0x80`, AUDIT_ARCH_I386) mata al proceso; una
/// llamada de x32 (el bit 0x40000000 en el numero, misma arquitectura) da EPERM. Se instala
/// en el hijo, despues de fork, con un solo hilo: lo heredan todos los hilos que node cree
/// despues del exec. `pub(crate)`: lo prueban sin node los tests del sandbox.
pub(crate) unsafe fn encerrar_senales() -> std::io::Result<()> {
    use libc::{sock_filter, sock_fprog, BPF_ABS, BPF_JEQ, BPF_JGE, BPF_JGT, BPF_JMP, BPF_K, BPF_LD, BPF_RET, BPF_W};
    // linux/audit.h (MEDIDO en /usr/include/linux/audit.h): EM_* | __AUDIT_ARCH_64BIT | __AUDIT_ARCH_LE.
    #[cfg(target_arch = "x86_64")]
    const ARCH_PROPIA: Option<u32> = Some(62 | 0x8000_0000 | 0x4000_0000);
    #[cfg(target_arch = "aarch64")]
    const ARCH_PROPIA: Option<u32> = Some(183 | 0x8000_0000 | 0x4000_0000);
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    const ARCH_PROPIA: Option<u32> = None;
    // Sin reservar memoria (entre fork y exec): ENOSYS, y `probar_sandbox` corta la corrida.
    let Some(arch) = ARCH_PROPIA else {
        return Err(std::io::Error::from_raw_os_error(libc::ENOSYS));
    };
    // Desplazamientos en `struct seccomp_data` (nr, arch, ip, args[6]); la mitad baja de
    // args[0] y args[1] (little endian en las dos arquitecturas): pid_t es de 32 bits, y
    // `setpriority(int which, int who, int prio)` lee `which` y `who` como int.
    const NR: u32 = 0;
    const ARCH_OFF: u32 = 4;
    const ARG0: u32 = 16;
    const ARG1: u32 = 24;
    // x86_64: las llamadas de x32 llevan este bit en el numero.
    const X32: u32 = 0x4000_0000;
    // Indices de las instrucciones a las que se salta (ver el programa abajo).
    const KILL: usize = 13;
    const PROPIO: usize = 17;
    const PRIO: usize = 19;
    const NIEGA: usize = 24;
    const DEJA: usize = 25;
    const fn salto(desde: usize, hasta: usize) -> u8 {
        (hasta - desde - 1) as u8
    }
    const fn ins(code: u32, jt: u8, jf: u8, k: u32) -> sock_filter {
        sock_filter { code: code as u16, jt, jf, k }
    }
    const LD: u32 = BPF_LD | BPF_W | BPF_ABS;
    const JEQ: u32 = BPF_JMP | BPF_JEQ | BPF_K;
    const RET: u32 = BPF_RET | BPF_K;
    let yo = libc::getpid() as u32;
    let nr = |n: libc::c_long| n as u32;
    let mut prog: [sock_filter; 26] = [
        /* 0 */ ins(LD, 0, 0, ARCH_OFF),
        /* 1 */ ins(JEQ, 1, 0, arch),
        /* 2 */ ins(RET, 0, 0, libc::SECCOMP_RET_KILL_PROCESS),
        /* 3 */ ins(LD, 0, 0, NR),
        /* 4 */ ins(BPF_JMP | BPF_JGE | BPF_K, salto(4, NIEGA), 0, X32),
        /* 5 */ ins(JEQ, salto(5, KILL), 0, nr(libc::SYS_kill)),
        /* 6 */ ins(JEQ, salto(6, PROPIO), 0, nr(libc::SYS_tgkill)),
        /* 7 */ ins(JEQ, salto(7, PROPIO), 0, nr(libc::SYS_tkill)),
        /* 8 */ ins(JEQ, salto(8, PROPIO), 0, nr(libc::SYS_rt_sigqueueinfo)),
        /* 9 */ ins(JEQ, salto(9, PROPIO), 0, nr(libc::SYS_rt_tgsigqueueinfo)),
        /* 10 */ ins(JEQ, salto(10, NIEGA), 0, nr(libc::SYS_pidfd_send_signal)),
        /* 11 */ ins(JEQ, salto(11, PRIO), 0, nr(libc::SYS_setpriority)),
        /* 12 */ ins(RET, 0, 0, libc::SECCOMP_RET_ALLOW),
        /* 13 KILL */ ins(LD, 0, 0, ARG0),
        /* 14 */ ins(JEQ, salto(14, DEJA), 0, yo),
        /* 15 */ ins(JEQ, salto(15, DEJA), 0, 0),
        /* 16 */ ins(JEQ, salto(16, DEJA), salto(16, NIEGA), yo.wrapping_neg()),
        /* 17 PROPIO */ ins(LD, 0, 0, ARG0),
        /* 18 */ ins(JEQ, salto(18, DEJA), salto(18, NIEGA), yo),
        // `which` mayor que PRIO_PGRP (PRIO_USER, o uno que no existe): EPERM.
        /* 19 PRIO */ ins(LD, 0, 0, ARG0),
        /* 20 */ ins(BPF_JMP | BPF_JGT | BPF_K, salto(20, NIEGA), 0, libc::PRIO_PGRP),
        /* 21 */ ins(LD, 0, 0, ARG1),
        /* 22 */ ins(JEQ, salto(22, DEJA), 0, yo),
        /* 23 */ ins(JEQ, salto(23, DEJA), 0, 0),
        /* 24 NIEGA */ ins(RET, 0, 0, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
        /* 25 DEJA */ ins(RET, 0, 0, libc::SECCOMP_RET_ALLOW),
    ];
    let fprog = sock_fprog { len: prog.len() as libc::c_ushort, filter: prog.as_mut_ptr() };
    // prctl es variadica y el kernel lee unsigned long: los argumentos van como c_ulong.
    let (uno, cero): (libc::c_ulong, libc::c_ulong) = (1, 0);
    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, uno, cero, cero, cero) != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let modo = libc::c_ulong::from(libc::SECCOMP_MODE_FILTER);
    if libc::prctl(libc::PR_SET_SECCOMP, modo, &fprog as *const sock_fprog) != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// n-sandbox: escribe `datos` en `ruta` (los mapas de uid/gid). Solo llamadas al sistema.
unsafe fn escribir(ruta: &std::ffi::CStr, datos: &[u8]) -> std::io::Result<()> {
    let fd = libc::open(ruta.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let n = libc::write(fd, datos.as_ptr().cast(), datos.len());
    let e = std::io::Error::last_os_error();
    libc::close(fd);
    if usize::try_from(n).ok() != Some(datos.len()) {
        return Err(e);
    }
    Ok(())
}

/// n-sandbox: `lo` arriba en el namespace de red nuevo (nace abajo: MEDIDO con `ip link`).
unsafe fn prender_loopback() -> std::io::Result<()> {
    let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut ifr: libc::ifreq = std::mem::zeroed();
    ifr.ifr_name[0] = b'l' as libc::c_char;
    ifr.ifr_name[1] = b'o' as libc::c_char;
    let mut r = libc::ioctl(fd, libc::SIOCGIFFLAGS, &mut ifr as *mut libc::ifreq);
    if r == 0 {
        ifr.ifr_ifru.ifru_flags |= libc::IFF_UP as libc::c_short;
        r = libc::ioctl(fd, libc::SIOCSIFFLAGS, &ifr as *const libc::ifreq);
    }
    let e = std::io::Error::last_os_error();
    libc::close(fd);
    if r != 0 {
        return Err(e);
    }
    Ok(())
}

/// n-sandbox: los sockets unix con ruta que hay ahora en el sistema (`/proc/net/unix`,
/// del namespace de red de difuzz), fuera de /run (que se tapa entero). Se arma en cada
/// ejecucion: un socket nuevo (otro programa que arranco despues) tambien se tapa. Limite:
/// una ruta con espacios no se lee entera y queda sin tapar.
fn sockets_del_sistema() -> Vec<std::ffi::CString> {
    use std::os::unix::fs::FileTypeExt;
    let Ok(texto) = std::fs::read_to_string("/proc/net/unix") else {
        return Vec::new();
    };
    let mut v: Vec<std::ffi::CString> = texto
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().nth(7))
        .filter(|p| p.starts_with('/') && *p != "/run" && !p.starts_with("/run/"))
        .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_socket()))
        .filter_map(|p| std::ffi::CString::new(p).ok())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// n-sandbox: antes de la corrida, que node arranque con el encierro pedido (node con
/// `--permission`, una senal a difuzz negada con EPERM y, con la red, los namespaces y nada
/// mas que `lo`). Si no, la corrida no
/// empieza: un sandbox que no se puede armar no se reemplaza en silencio por ninguno.
pub fn probar_sandbox(s: Sandbox) -> Result<(), String> {
    if !s.activo() {
        return Ok(());
    }
    // 2b: la senal 0 a difuzz (el padre: sin el filtro llega, mismo usuario) tiene que dar EPERM.
    let js = "const i = Object.keys(require('os').networkInterfaces());\
              let senal = 'paso'; try { process.kill(process.ppid, 0); } catch (e) { senal = e.code; }\
              process.exitCode = typeof process.permission !== 'object' ? 3 : (process.argv[1] === 'red' && i.some((n) => n !== 'lo')) ? 4 : senal !== 'EPERM' ? 5 : 0;";
    let mut cmd = Command::new(node_path());
    cmd.env_clear()
        .arg("--permission")
        .arg("-e")
        .arg(js)
        .arg(if s == Sandbox::PermisosYRed { "red" } else { "permisos" })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let aislar = (s == Sandbox::PermisosYRed).then(Aislar::armar);
    // SAFETY: como en run_file.
    unsafe {
        cmd.pre_exec(move || encerrar(aislar.as_ref()));
    }
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!(
            "node con --sandbox ({}) no arranco: {} {}",
            s.nombre(),
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Err(format!("node con --sandbox ({}) no arranco: {e}", s.nombre())),
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

/// 11.7 (I5): la salida de una ejecucion contra el tope (`Runner::tope_salida`): los bytes leidos de
/// los dos pipes y si ya pasaron el tope.
#[derive(Default)]
struct Salida {
    /// En bytes; 0, sin tope.
    tope: u64,
    usado: std::sync::atomic::AtomicU64,
    excedida: std::sync::atomic::AtomicBool,
}

/// Lee `pipe` hasta EOF en un hilo propio, acumulando en un buffer compartido.
/// Avisa por `done` al llegar al EOF (o a un error de lectura).
/// 11.7 (I5): lo que pasa el tope de `salida` (sumando los dos pipes) ya no se guarda: se sigue
/// leyendo y tirando hasta el EOF (para que node no se trabe escribiendo antes de que lo maten) y
/// queda marcada (`Salida::excedida`).
fn drain(mut pipe: impl Read + Send + 'static, done: mpsc::Sender<()>, salida: Arc<Salida>) -> Arc<Mutex<Vec<u8>>> {
    use std::sync::atomic::Ordering;
    let buf = Arc::new(Mutex::new(Vec::new()));
    let b = Arc::clone(&buf);
    std::thread::spawn(move || {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let total = salida.usado.fetch_add(n as u64, Ordering::SeqCst) + n as u64;
                    if salida.tope > 0 && total > salida.tope {
                        salida.excedida.store(true, Ordering::SeqCst);
                    } else {
                        b.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
        let _ = done.send(());
    });
    buf
}

/// 11.5 (I3): desenrolla con `Excedido` (ver ahi). `script` es el archivo que corrio node (el
/// programa, o el envoltorio observador que lo importa); el texto que se guarda es el del programa.
fn exceder(r: &Runner, script: &Path, causa: &'static str, que: String) -> ! {
    // Sin la ruta: lleva el pid de difuzz y el worker.
    let de = if script == r.script { "del programa (programa.*)" } else { "del envoltorio observador que importa el programa (programa.*)" };
    std::panic::resume_unwind(Box::new(Excedido {
        causa,
        mensaje: format!("{causa}: la ejecucion de node {de} {que}"),
        programa: std::fs::read_to_string(&r.script).unwrap_or_default(),
        ext: r.kind.ext(),
    }))
}

/// 11.6 (I4): el `Command` de node, que muere con SIGKILL cuando muere el hilo de difuzz que lo
/// lanzo (`PR_SET_PDEATHSIG`; es por hilo, no por proceso: MEDIDO en `pdeathsig_sigue_al_hilo`). En
/// difuzz node lo lanza el worker y `run_file` no vuelve hasta cosecharlo, asi que el hilo vive mas
/// que node salvo que muera el proceso entero: SIGKILL, el OOM killer o `_exit` tras la segunda
/// senal. Antes, ese node quedaba huerfano (PPID 1) y un `while (true) {}` giraba para siempre. No
/// cubre a los procesos que lance el programa (sin `--sandbox`): no heredan la senal.
///
/// Con `setpriv` (util-linux, `setpriv()`), node se lanza como `setpriv --pdeathsig KILL -- node
/// ...`: setpriv pone la senal y hace exec de node en el mismo proceso (mismo pid, mismo grupo;
/// node ve el mismo argv y el mismo ppid). Si no, el prctl va en `pre_exec` (`prctl_en_el_hijo`,
/// lo pide el segundo valor: `true`). La diferencia es el costo: sin pre_exec propio std lanza con
/// posix_spawn (vfork); con pre_exec, con fork, y mientras el hijo no hizo exec el padre paga
/// copy-on-write en todos sus hilos (MEDIDO en 300 casos del corpus SWC, `--jobs 8`, mediana de 6
/// pares intercalados: fork +23 % de tiempo de sistema y +8 % de pared; setpriv +3 % y +2 %).
fn comando_node(setpriv: Option<&Path>) -> (Command, bool) {
    match setpriv {
        Some(s) => {
            let mut c = Command::new(s);
            c.args(["--pdeathsig", "KILL", "--"]).arg(node_path());
            (c, false)
        }
        None => (Command::new(node_path()), true),
    }
}

/// 11.6 (I4): `setpriv` del PATH de difuzz si acepta `--pdeathsig` (util-linux >= 2.33; se prueba
/// una vez, lanzando `node --version` a traves de el). `None`: el prctl va en pre_exec.
fn setpriv() -> Option<&'static Path> {
    static SETPRIV: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    SETPRIV
        .get_or_init(|| {
            let p = std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join("setpriv")).find(|p| p.is_file())?;
            let (mut c, _) = comando_node(Some(&p));
            c.arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            c.status().is_ok_and(|s| s.success()).then_some(p)
        })
        .as_deref()
}

/// 11.6 (I4): el prctl de `comando_node` en el hijo, entre fork y exec (sin `setpriv`, o con el
/// sandbox, que ya lanza con fork).
fn prctl_en_el_hijo(cmd: &mut Command) {
    let padre = libc::pid_t::try_from(std::process::id()).unwrap_or(0);
    // SAFETY: `morir_con_el_padre` corre entre fork y exec y solo hace llamadas al sistema (prctl,
    // getppid) sin reservar memoria ni tomar locks.
    unsafe {
        cmd.pre_exec(move || morir_con_el_padre(padre));
    }
}

/// 11.6 (I4): `PR_SET_PDEATHSIG` en SIGKILL. Si difuzz ya habia muerto entre el fork y el prctl (la
/// senal no llegaria nunca), no se lanza (con `setpriv` esa ventana no se cierra: son microsegundos).
fn morir_con_el_padre(padre: libc::pid_t) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    // SAFETY: prctl y getppid con enteros; no tocan memoria de este proceso.
    unsafe {
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::getppid() != padre {
            return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = padre;
    Ok(())
}

/// 11.6 (I4): los grupos de los node que estan corriendo (pgid = pid de node, `process_group(0)`),
/// para `matar_vivos`. Un arreglo fijo de atomicos: el manejador de senales no puede reservar ni
/// tomar locks. Si se llenara (mas de `MAX_VIVOS` ejecuciones a la vez), la que no entra queda solo
/// con `PR_SET_PDEATHSIG`.
const MAX_VIVOS: usize = 1024;
static VIVOS: [std::sync::atomic::AtomicI32; MAX_VIVOS] = [const { std::sync::atomic::AtomicI32::new(0) }; MAX_VIVOS];

/// 11.6 (I4): el lugar de un node en `VIVOS`; se libera al soltarse (despues de cosecharlo).
struct Vivo(Option<usize>);

impl Vivo {
    fn registrar(pid: u32) -> Vivo {
        use std::sync::atomic::Ordering;
        let Ok(pid) = i32::try_from(pid) else {
            return Vivo(None);
        };
        Vivo(VIVOS.iter().position(|v| v.compare_exchange(0, pid, Ordering::SeqCst, Ordering::SeqCst).is_ok()))
    }
}

impl Drop for Vivo {
    fn drop(&mut self) {
        if let Some(i) = self.0 {
            VIVOS[i].store(0, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// 11.6 (I4): SIGKILL al grupo de cada node en curso. Lo llama el manejador de la segunda senal
/// (`main::on_signal`) antes del `_exit`: solo lee atomicos y llama a kill(2), que es
/// async-signal-safe. Mata tambien a lo que el programa lanzo en su grupo (eso `PR_SET_PDEATHSIG` no
/// lo cubre); lo que hizo `setsid` (el subreaper lo cosecha despues de cada ejecucion) queda.
pub fn matar_vivos() {
    for v in &VIVOS {
        let pid = v.load(std::sync::atomic::Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: kill(2) con enteros.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

/// 11.5 (I3): node se espera con `wait4` para tener su pico de RSS. Desde que se cosecha no se
/// toca su pid (podria reusarse).
struct Cosecha {
    pid: libc::pid_t,
    wst: libc::c_int,
    uso: libc::rusage,
    cosechado: bool,
}

impl Cosecha {
    fn new(pid: u32) -> Cosecha {
        Cosecha {
            pid: libc::pid_t::try_from(pid).unwrap_or(libc::pid_t::MAX),
            wst: 0,
            // SAFETY: `rusage` es una estructura de enteros: todo en cero es valido.
            uso: unsafe { std::mem::zeroed() },
            cosechado: false,
        }
    }

    /// `wait4` del hijo, sin bloquear (`bloquear` false) o hasta que termine. `Ok(true)`: ya esta
    /// cosechado (ahora o antes).
    fn esperar(&mut self, bloquear: bool) -> std::io::Result<bool> {
        while !self.cosechado {
            // SAFETY: punteros a campos propios, vivos durante la llamada.
            let w = unsafe { libc::wait4(self.pid, &mut self.wst, if bloquear { 0 } else { libc::WNOHANG }, &mut self.uso) };
            if w == self.pid {
                self.cosechado = true;
            } else if w == 0 {
                return Ok(false);
            } else {
                let e = std::io::Error::last_os_error();
                if e.kind() != std::io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
        }
        Ok(true)
    }

    /// SIGKILL al grupo y a node, y lo cosecha (si todavia no estaba cosechado).
    fn matar(&mut self) {
        if self.cosechado {
            return;
        }
        kill_group(self.pid as u32);
        // SAFETY: kill(2) no toca memoria; el pid es de un hijo sin cosechar (no se reuso).
        unsafe {
            libc::kill(self.pid, libc::SIGKILL);
        }
        let _ = self.esperar(true);
    }

    /// Como termino (solo despues de `esperar` en `Ok(true)`).
    fn status(&self) -> Status {
        if libc::WIFEXITED(self.wst) {
            Status::Exited(libc::WEXITSTATUS(self.wst))
        } else {
            Status::Signal
        }
    }

    /// El pico de RSS de node (`ru_maxrss`, en KiB en Linux), en bytes; 0 si no se cosecho.
    fn pico_rss(&self) -> u64 {
        if !self.cosechado {
            return 0;
        }
        u64::try_from(self.uso.ru_maxrss).unwrap_or(0).saturating_mul(1024)
    }
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

    /// 7.4: la corrida sin el sensor hereda las lineas `viol` y los flags de violacion
    /// atrapada de la corrida con el sensor, sin repetir las que ya tiene; nada mas.
    #[test]
    fn heredar_violaciones_del_sensor() {
        use super::{Exec, Status};
        use std::time::Duration;
        let ex = |ctl: &str, flags: &[&str]| Exec {
            status: Status::Exited(0),
            stdout: "1\n".into(),
            stderr: String::new(),
            flags: flags.iter().map(|f| f.to_string()).collect(),
            elapsed: Duration::ZERO,
            ctl: ctl.into(),
        };
        let con = ex(
            "viol tdz 1:6\nviol tdz 1:6\nviol symbol 3:2\nviol tdz 7:0\nsensor_tope\nflags builtins,coercion_symbol,stack,tdz,tdz_atrapada\n",
            &["builtins", "coercion_symbol", "stack", "tdz", "tdz_atrapada"],
        );
        // La no atrapada (`tdz 7:0`, flag `tdz`) la ve tambien la corrida sin el sensor.
        let mut sin = ex("viol tdz 7:0\nflags tdz", &["tdz"]);
        assert_eq!(sin.heredar_violaciones(&con), ["tdz 1:6", "symbol 3:2"]);
        assert_eq!(sin.ctl, "viol tdz 7:0\nflags tdz\nviol tdz 1:6\nviol symbol 3:2\n");
        // Solo los de violacion: `builtins` y `stack` son de la corrida con el sensor.
        assert_eq!(sin.flags, ["coercion_symbol", "tdz", "tdz_atrapada"]);
        assert_eq!(crate::recheck::violaciones(&sin), ["tdz 7:0", "tdz 1:6", "symbol 3:2"]);
        // Heredar dos veces no cambia nada; sin violaciones, nada que heredar.
        assert!(sin.heredar_violaciones(&con).is_empty());
        assert_eq!(sin.flags, ["coercion_symbol", "tdz", "tdz_atrapada"]);
        let mut otra = ex("", &[]);
        assert!(otra.heredar_violaciones(&ex("sensor_tope\nflags \n", &[])).is_empty());
        assert_eq!((otra.ctl.as_str(), otra.flags.len()), ("", 0));
    }

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

        if !crate::test_util::requiere_node() {
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let runner = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20)).with_kind(SourceKind::Module);

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
        // 11.2: ni es un codigo propio comparable (main lo descarta como `enlace`).
        assert!(link.codigo_propio().is_none() && !link.ok_with_output() && !link.ok_silent());
        // Un throw comun sigue siendo THROW_EXIT, con flags.
        let thr = runner.run("throw new TypeError('x');\n");
        assert!(thr.threw());
        assert_eq!(thr.stdout, "!dz throw TypeError\n");
    }

    /// M2: un hijo que hereda el stdout no estira el timeout ni queda huerfano.
    /// 9d: sin cota absoluta (la de 1500 ms fallaba bajo carga). Si el runner no
    /// matara el grupo, el `sleep` retendria el pipe y el runner esperaria
    /// READ_GRACE (1 s) de mas: se compara contra el mismo timeout sin hijo,
    /// medido antes y despues de cada intento, con margen READ_GRACE / 2. Hasta 3
    /// intentos (un pico de carga es aleatorio; esperar READ_GRACE es sistematico
    /// y falla en los 3). Que el `sleep` no sobreviva se exige en cada intento.
    /// 10.6: con carga alta (load 42-48) node no llegaba a lanzar el `sleep` ni a
    /// imprimir su pid antes del timeout de 500 ms ("sin pid del sleep: \"\""). Ese
    /// intento no mide nada: se repite con el doble de timeout, hasta 4 veces (8 s);
    /// no cuenta entre los 3.
    /// 10.2b (B-H3 de la revision B del bloque 10): el `sleep` duraba 5 s fijos y con el timeout
    /// escalado a 8 s terminaba solo antes del kill: con `kill_group` roto el test pasaba. Ahora
    /// dura `6 x timeout + 5 s` (una corrida colgada puede tardar dos timeouts: la repeticion sin
    /// el sensor) y el `sleep` se mira apenas termina la corrida que lo lanzo.
    #[test]
    fn timeout_mata_el_grupo_y_no_espera_al_hijo() {
        use super::{Preload, Runner, Status, READ_GRACE};
        use std::time::{Duration, Instant};

        if !crate::test_util::requiere_node() {
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m2.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let medir = |runner: &Runner, js: &str| {
            let t0 = Instant::now();
            let e = runner.run(js);
            assert_eq!(e.status, Status::Timeout, "{js}");
            (e, t0.elapsed())
        };
        let vivo = |pid: i32| std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.starts_with(b"sleep\0"));
        let sin_hijo = "while(true){}\n";
        let con_hijo = |timeout: Duration| {
            let s = (timeout.as_secs_f64() * 6.0).ceil() as u64 + 5;
            format!("const c = require('child_process').spawn('sleep',['{s}'],{{stdio:'inherit'}});\nconsole.log(c.pid);\nwhile(true){{}}\n")
        };
        let mut tiempos = Vec::new();
        let (mut timeout, mut sin_pid) = (Duration::from_millis(500), Vec::new());
        while tiempos.len() < 3 {
            let runner = Runner::new(&preload, crate::test_util::id_runner(), timeout);
            let (_, antes) = medir(&runner, sin_hijo);
            let (e, t) = medir(&runner, &con_hijo(timeout));
            let Ok(pid) = e.stdout.trim().parse::<i32>() else {
                sin_pid.push((timeout, e.stdout));
                assert!(sin_pid.len() <= 4, "node no llego a imprimir el pid del sleep antes del timeout: {sin_pid:?}");
                timeout *= 2;
                continue;
            };
            assert!(!vivo(pid), "el sleep {pid} sigue vivo (timeout {timeout:?}): el grupo no se mato");
            let (_, despues) = medir(&runner, sin_hijo);
            let base = (antes + despues) / 2;
            tiempos.push((t, base));
            if t < base + READ_GRACE / 2 {
                break;
            }
        }
        let (t, base) = *tiempos.last().unwrap();
        assert!(t < base + READ_GRACE / 2, "espero al hijo en los 3 intentos (tiempo, base): {tiempos:?} (sin pid: {sin_pid:?})");
    }

    /// Paso 7 (revision-tint I1): ante Timeout con el sensor, la repeticion sin el usa
    /// el timeout entero (como en b4c98fd). Con la sonda de M2 (un cuarto) un programa
    /// lento por el sensor y que sin el tarda mas que la sonda quedaba en Timeout y se
    /// perdia un div/ real (corpus-sonda/t01). El costo: un cuelgue de verdad paga dos
    /// timeouts.
    #[test]
    fn timeout_con_sensor_repite_con_el_timeout_entero() {
        use super::{Preload, Runner, Status};
        use std::time::{Duration, Instant};

        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("i1-timeout").unwrap();
        let t = Duration::from_millis(1500);
        let r = Runner::new(&preload, crate::test_util::id_runner(), t);
        // Cuelga de verdad: dos timeouts enteros.
        let t0 = Instant::now();
        let e = r.run("try { null.x; } catch (e) {}\nconsole.log('antes');\nfor (;;) {}\n");
        let dt = t0.elapsed();
        assert_eq!(e.status, Status::Timeout);
        assert!(e.ctl.starts_with("sensor_caido\n"), "{}", e.ctl);
        assert!(dt >= t + t, "la repeticion sin sensor no uso el timeout entero: {dt:?}");
    }

    /// M9: en el carril A, un require no atrapado de algo que no existe es
    /// enlace (LINK_EXIT), no un throw comparable. Atrapado, sigue siendo programa.
    #[test]
    fn require_inexistente_es_enlace() {
        use super::{Preload, Runner, Status, LINK_EXIT};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m9.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let runner = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
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
        if !crate::test_util::requiere_node() {
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m10.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
        // exit 70 propio: no lanzo, no termino "solo".
        let e = r.run("console.log(1);\nprocess.exit(70);\n");
        assert_eq!(e.status, Status::Exited(70));
        assert!(!e.threw() && !e.completed());
        // 11.2: pero es un codigo propio, comparable (con salida).
        assert!(e.codigo_propio() == Some(70) && e.ok_with_output() && !e.ok_silent());
        let e = r.run("process.exitCode = 70;\n");
        assert!(!e.threw() && !e.completed());
        assert!(e.codigo_propio() == Some(70) && e.ok_silent());
        // exit 71 propio: no es enlace.
        let e = r.run("process.exit(71);\n");
        assert!(!e.link_failed() && e.codigo_propio() == Some(71));
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
        if !crate::test_util::requiere_node() {
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m11.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let id = crate::test_util::id_runner();
        let r = Runner::new(&preload, id, Duration::from_secs(20));
        let js = format!("const fs = require('fs');\nconsole.log(fs.existsSync('x.txt'), process.cwd().endsWith('-w{id}-cwd'), process.env.HOME === process.cwd(), Object.keys(process.env).sort().join());\nfs.writeFileSync('x.txt', '1');\n");
        let e1 = r.run(&js);
        assert_eq!(e1.status, Status::Exited(0), "{}", e1.stderr);
        assert_eq!(e1.stdout, "false true true HOME,PATH,TMPDIR,TZ\n");
        assert_eq!(r.run(&js).stdout, e1.stdout);
        drop(r);
        assert!(!super::run_dir().join(format!("difuzz-{}-w{id}-cwd", std::process::id())).exists());
    }

    /// M12: un modulo mudo sin exports se observa con el epilogo (parseado como
    /// modulo), y `--invocar` llama a las funciones exportadas.
    #[test]
    fn carril_b_epilogo_e_invocar_exports() {
        use super::{Preload, Runner, Status};
        use crate::minify::{instrument, SourceKind};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m12.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20)).with_kind(SourceKind::Module);
        let mudo = "const o = { a: 1 };\nlet n = await Promise.resolve(2);\nfunction f() { return n; }\n";
        let epi = instrument(mudo, false, SourceKind::Module).expect("un modulo con bindings se observa");
        let e = r.run(&epi);
        assert_eq!(e.status, Status::Exited(0), "{}", e.stderr);
        assert_eq!(e.stdout, "o = {\"a\": 1}\nn = 2\nf = [Function: f]\n");
        // `import` no es un binding que observar; un script no parsea como modulo.
        assert!(instrument("import 'node:fs';\n", false, SourceKind::Module).is_none());
        let inv = r.with_invocar(true);
        let e = inv.observing().run("export function f() { return 2; }\nexport const k = 3;\n");
        assert_eq!(e.stdout, "export f = [Function: f]\nexport k = 3\nexport f() = 2\n");
    }

    /// M16: los argumentos extra llegan a node.
    #[test]
    fn argumentos_extra_de_node() {
        use super::{Preload, Runner};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m16.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
        let js = "console.log(process.execArgv.filter((a) => a.startsWith('--stack')).join());\n";
        assert_eq!(r.run(js).stdout, "\n");
        assert_eq!(r.run_with_args(js, &[super::DEEP_STACK]).stdout, format!("{}\n", super::DEEP_STACK));
    }

    /// M20: pid, hrtime, crypto, memoria y uptime son deterministas.
    #[test]
    fn preload_fija_lo_no_determinista() {
        use super::{Preload, Runner, Status};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let path = std::env::temp_dir().join(format!("difuzz-test-{}-preload-m20.cjs", std::process::id()));
        std::fs::write(&path, super::PRELOAD).unwrap();
        let preload = Preload { path };
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
        let js = "const c = require('crypto'), os = require('os');\nconst t0 = process.hrtime();\nconsole.log(process.pid, process.ppid, process.hrtime(t0), process.hrtime.bigint(), process.uptime());\nconsole.log(c.randomUUID(), crypto.randomUUID(), c.randomBytes(4).toString('hex'), crypto.getRandomValues(new Uint8Array(3)).join(), c.randomInt(10));\nconsole.log(process.memoryUsage().rss, process.memoryUsage.rss(), os.freemem(), os.uptime());\n";
        let a = r.run(js);
        assert_eq!(a.status, Status::Exited(0), "{}", a.stderr);
        assert_eq!(r.run(js).stdout, a.stdout);
        assert!(a.stdout.starts_with("4204242 4204241 [ 0, 1000000 ] 3000000n 1\n"), "{}", a.stdout);
    }

    /// 7.2: sin sandbox, `process.kill` con el pid ficticio llega al propio node (y nunca a
    /// otro proceso: un senuelo propio sigue vivo), con el ppid ficticio da ESRCH sin llamar
    /// al sistema, y `process._kill` envuelto se ve como el nativo. Los destinos que no son
    /// node solo reciben la senal 0.
    #[test]
    fn kill_con_el_pid_ficticio() {
        use super::{Preload, Runner, Status};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        // Ningun proceso de esta maquina puede tener el pid ni el ppid ficticios (los pids
        // van de 1 a pid_max - 1; preload_fija_lo_no_determinista mira que sean estos).
        let pid_max: u32 = std::fs::read_to_string("/proc/sys/kernel/pid_max").unwrap().trim().parse().unwrap();
        assert!(4204241 >= pid_max, "pid_max {pid_max}");
        let preload = Preload::write_tagged("kill-ficticio").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
        let mut senuelo = std::process::Command::new("sleep").arg("60").spawn().unwrap();
        let t = "const t = (f) => { try { return String(f()); } catch (e) { return `${e.code} ${e.syscall} ${e.errno} ${e.message}`; } };\n";
        // Lo que el programa ve de `_kill`, de `kill` y de los errores.
        let js = format!(
            "{t}const k = process._kill, d = Object.getOwnPropertyDescriptor(process, '_kill');\n\
             console.log(Function.prototype.toString.call(k), k.length, k.name, 'prototype' in k, Object.getOwnPropertyNames(k).join(), d.writable, d.enumerable, d.configurable, t(() => new k(1, 0)));\n\
             console.log(process.kill.toString().startsWith('function kill(pid, sig)'), process.kill.length);\n\
             console.log(t(() => process.kill(process.pid, 0)), t(() => process.kill(-process.pid, 0)), t(() => process.kill(0, 0)), t(() => k(process.pid, 0)), t(() => k(process.pid + 2 ** 32, 0)));\n\
             console.log(t(() => process.kill(process.ppid, 0)), t(() => process.kill(-process.ppid, 0)), t(() => k(process.ppid, 999)), t(() => process.kill(process.ppid, 'SIGFOO')));\n\
             let orden = []; t(() => k({{ valueOf() {{ orden.push('pid'); return process.pid; }} }}, {{ valueOf() {{ orden.push('sig'); return 0; }} }})); console.log(orden.join());\n"
        );
        let e = r.run(&js);
        assert_eq!(e.status, Status::Exited(0), "{}", e.stderr);
        assert_eq!(
            e.stdout,
            "function _kill() { [native code] } 0 _kill false length,name true true true undefined undefined undefined k is not a constructor\n\
             true 2\n\
             true true true 0 0\n\
             ESRCH kill -3 kill ESRCH ESRCH kill -3 kill ESRCH -3 ERR_UNKNOWN_SIGNAL undefined undefined Unknown signal: SIGFOO\n\
             pid,sig\n"
        );
        // A si mismo: SIGTERM sin handler mata a node (como node sin el preload: MEDIDO, 143).
        let e = r.run("process.kill(process.pid, 'SIGTERM');\nconsole.log('sigue');\n");
        assert_eq!((e.status.clone(), e.stdout.as_str()), (Status::Signal, ""), "{}", e.stderr);
        // Con un handler propio la recibe (el timer mantiene vivo el loop hasta que llega).
        let e = r.run("process.on('SIGTERM', () => console.log('recibida'));\nprocess.kill(process.pid, 'SIGTERM');\nsetTimeout(() => {}, 50);\n");
        assert_eq!((e.status.clone(), e.stdout.trim()), (Status::Exited(0), "recibida"), "{}", e.stderr);
        // Por `_kill` directo y al grupo propio (el negativo del pid ficticio), igual.
        let e = r.run("process.on('SIGUSR2', () => console.log('grupo'));\nprocess._kill(-process.pid, 12);\nsetTimeout(() => {}, 50);\n");
        assert_eq!((e.status.clone(), e.stdout.trim()), (Status::Exited(0), "grupo"), "{}", e.stderr);
        // `_debugProcess` (SIGUSR1 al pid que recibe, MEDIDO con strace) con el pid ficticio no
        // llega a nadie: ningun proceso lo tiene.
        let e = r.run(&format!("{t}console.log(t(() => process._debugProcess(process.pid)));\n"));
        assert_eq!(e.stdout, "ESRCH kill 3 ESRCH, No such process\n");
        assert!(senuelo.try_wait().unwrap().is_none(), "el senuelo recibio una senal");
        let _ = senuelo.kill();
        let _ = senuelo.wait();
    }

    /// n-ruido (t-real): los timers de node salen opacos (su estado trae el reloj real
    /// del event loop: `_idleStart`, `expiry`); una clase del programa llamada Timeout no.
    #[test]
    fn timers_de_node_opacos() {
        use super::{Preload, Runner, Status};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("timers").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
        let js = "const t = setTimeout(() => {}, 5), v = setInterval(() => {}, 7), i = setImmediate(() => {});\nv.resultado = \"PASS\";\nclass Timeout { constructor() { this.x = 1; } }\n__difuzzObserve([\"t\", t], [\"v\", v], [\"i\", i], [\"o\", { t, d: { i } }], [\"c\", new Timeout()]);\nclearTimeout(t); clearInterval(v); clearImmediate(i);\n";
        let a = r.run(js);
        assert_eq!(a.status, Status::Exited(0), "{}", a.stderr);
        // b7-3: se ocultan el reloj, la lista enlazada y los ids de async_hooks; el resto
        // (delay, `_repeat`, `_destroyed`, refed, lo agregado) se imprime.
        let t = "[Timeout]{\"_idleTimeout\": 5, \"_onTimeout\": [Function (anonymous)], \"_timerArgs\": undefined, \"_repeat\": null, \"_destroyed\": false, [Symbol(refed)]: true, [Symbol(kHasPrimitive)]: false, [Symbol(kAsyncContextFrame)]: undefined}";
        let v = "[Timeout]{\"_idleTimeout\": 7, \"_onTimeout\": [Function (anonymous)], \"_timerArgs\": undefined, \"_repeat\": 7, \"_destroyed\": false, \"resultado\": \"PASS\", [Symbol(refed)]: true, [Symbol(kHasPrimitive)]: false, [Symbol(kAsyncContextFrame)]: undefined}";
        let i = "[Immediate]{\"_onImmediate\": [Function (anonymous)], \"_argv\": undefined, \"_destroyed\": false, [Symbol(refed)]: true, [Symbol(kAsyncContextFrame)]: undefined}";
        assert_eq!(a.stdout, format!("t = {t}\nv = {v}\ni = {i}\no = {{\"t\": {t}, \"d\": {{\"i\": {i}}}}}\nc = inst{{\"x\": 1}}\n"));
    }

    /// b7-3 (revision n2, MENOR 4): lo que el programa hace con un timer se ve (setInterval o
    /// setTimeout, delay, cancelado o no, unref, argumentos, `+t`); el reloj no (la misma
    /// fuente con mas o menos espera antes de armar los timers da la misma salida).
    #[test]
    fn timers_de_node_muestran_el_comportamiento() {
        use super::{Preload, Runner, Status};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("timers-comp").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
        let correr = |js: &str| {
            let e = r.run(js);
            assert_eq!(e.status, Status::Exited(0), "{js}: {}", e.stderr);
            e.stdout
        };
        // Cada variante cambia una cosa del programa: todas las salidas son distintas.
        let base = "var t = setTimeout(function () {}, 50, 1);\nvar i = setImmediate(function () {});\n";
        let variantes = [
            base.to_string(),
            base.replace("setTimeout", "setInterval"),
            base.replace("50", "60"),
            base.replace(", 1)", ", 2)"),
            format!("{base}clearTimeout(t);\n"),
            format!("{base}t.unref();\n"),
            format!("{base}+t;\n"),
            format!("{base}clearImmediate(i);\n"),
            format!("{base}i.unref();\n"),
        ];
        let obs = "__difuzzObserve([\"t\", t], [\"i\", i]);\nclearTimeout(t); clearImmediate(i);\n";
        let salidas: Vec<String> = variantes.iter().map(|v| correr(&format!("{v}{obs}"))).collect();
        for (k, s) in salidas.iter().enumerate() {
            assert!(!s.contains("_idleStart") && !s.contains("expiry") && !s.contains("asyncId"), "{s}");
            for (j, o) in salidas.iter().enumerate().skip(k + 1) {
                assert_ne!(s, o, "variantes {k} y {j}:\n{}\n{}", variantes[k], variantes[j]);
            }
        }
        assert!(salidas[1].contains("\"_repeat\": 50"), "{}", salidas[1]);
        // El reloj: la misma fuente, con una espera de ~30 ms antes (sin salida ni bindings
        // observados) y con dos timers del mismo delay en la lista, da lo mismo.
        let espera = "(function () { for (var q = 0; q < 3e7; q++); })();\n";
        let par = "var a = setTimeout(function () {}, 70), b = setTimeout(function () {}, 70);\n__difuzzObserve([\"a\", a], [\"b\", b]);\nclearTimeout(a); clearTimeout(b);\n";
        assert_eq!(correr(&format!("{espera}{base}{obs}")), salidas[0]);
        assert_eq!(correr(&format!("{espera}{par}")), correr(par));
    }

    /// M21: el mensaje y el stack del throw llegan por el canal, sin prender el
    /// flag `stack` (lo lee el preload, no el programa).
    #[test]
    fn excepcion_con_mensaje_por_el_canal() {
        use super::{Preload, Runner};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("m21").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
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

    /// R1-H22: el directorio de los runners existe mientras viva alguno y el
    /// ultimo en irse lo borra si quedo vacio (con algo ajeno adentro, no). Con
    /// un contador propio sobre un directorio propio: el de `run_dir()` lo
    /// comparten los tests que corren en paralelo.
    #[test]
    fn el_ultimo_runner_borra_su_directorio() {
        let tmp = std::env::temp_dir().join(format!("difuzz-test-inquilinos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("run");
        let inq = super::Inquilinos::new();
        inq.entrar(&dir);
        inq.entrar(&dir);
        assert!(dir.is_dir());
        inq.salir(&dir);
        assert!(dir.is_dir(), "queda un inquilino: el directorio sigue");
        inq.salir(&dir);
        assert!(!dir.exists(), "el ultimo lo borra");
        inq.entrar(&dir);
        assert!(dir.is_dir(), "se vuelve a crear");
        std::fs::write(dir.join("ajeno"), "x").unwrap();
        inq.salir(&dir);
        assert!(dir.join("ajeno").exists(), "lo ajeno no se borra");
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

    /// t-sensor: el pre-filtro del sensor. Ante cualquier forma de atrapar (o de que
    /// el preload atrape codigo del programa), true.
    #[test]
    fn puede_atrapar_sobre_aproxima() {
        use super::puede_atrapar as p;
        for js in [
            "try { x; } catch {}\nlet x;\n",
            "f().catch(() => {});\n",
            "async function f() {}\n",
            "new Promise((r) => r());\n",
            "const p = fs.promises;\n",
            "x.then(f);\n",
            "import('./m.mjs');\n",
            "import /* c */ ('./m.mjs');\n",
            "queueMicrotask(f);\n",
            "new Proxy({}, {});\n",
            "class A extends B {}\n",
            "throw { get stack() { return 1; } };\n",
            "throw { toString() { return 1; } };\n",
            "Object.defineProperty(o, 'x', {});\n",
            "eval('1');\n",
            "var \\u0074ry2 = 1;\n",
            "const it = { next() {}, return() { z; } };\n",
            "const it = { next() {}, return(v) {\n z; } };\n",
            "const it = { next() {}, return(v = f(1)) { z; } };\n",
            "function f() { return (1 +\n",
            "const it = { next() {}, return: f };\n",
            "it.return = f;\n",
            "it[\"return\"] = f;\n",
            "let a = Symbol();\n",
            "// un try en un comentario tambien\n",
        ] {
            assert!(p(js), "{js}");
        }
        for js in [
            "console.log(1);\n",
            "x;\nlet x;\n",
            "function f(a) { return (a); }\nfunction g() { return f(1) + 2; }\n",
            "function f(a) { return (a(g(1)) + 1); }\n",
            "import x from './m.mjs';\nexport const y = x;\n",
            "var tryhard = 1, catcher = 2, thenable = 3;\n",
            "setTimeout(() => { f(); });\nprocess.nextTick(f);\n",
        ] {
            assert!(!p(js), "{js}");
        }
    }

    /// t-sensor: el sensor perezoso (un programa sin nada que atrape) da lo mismo
    /// que el sensor prendido desde el arranque. El mismo programa con `/* try */`
    /// al final (no mueve ninguna posicion) corre con el sensor prendido. Lo que
    /// atrapa `invocar` (Symbol, TDZ) se sigue viendo porque `invocar` lo prende.
    #[test]
    fn sensor_perezoso_igual_que_prendido() {
        use super::{puede_atrapar, Preload, Runner};
        use crate::minify::SourceKind;
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("perezoso").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
        let m = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20)).with_kind(SourceKind::Module);
        let mi = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20)).with_kind(SourceKind::Module).with_invocar(true);
        let mio = mi.observing();
        let viol = |e: &super::Exec| e.ctl.lines().filter(|l| l.starts_with("viol ")).map(String::from).collect::<Vec<_>>();
        // Paso 7 (I2): getOwnPropertySymbols y getPrototypeOf ya estan en la lista; un Symbol
        // sin ninguna palabra de la lista sale del nombre armado en runtime (riesgo anotado).
        let simbolo = "const s = globalThis[\"Sym\" + \"bol\"](\"s\");\n";
        let casos = [
            (&r, format!("{simbolo}function f() {{ return s + 1; }}\n__difuzzObserveInvocar([\"f\", f]);\n")),
            (&r, "function f() { return z; }\n__difuzzObserveInvocar([\"f\", f]);\nlet z = 1;\n".to_string()),
            (&r, "console.log(1);\nx;\nlet x;\n".to_string()),
            (&r, format!("{simbolo}console.log(1);\nMath.abs(s);\n")),
            (&r, "setTimeout(() => { f(); });\nfunction f() { z; let z; }\n".to_string()),
            (&mio, "export function f() { return z; }\nlet z = 1;\n".to_string()),
            (&mio, format!("{simbolo}export function f() {{ return s + 1; }}\n")),
            // Paso 7 (revision-tint I2, s03 sin palabras de la lista): la TDZ ocurre adentro
            // de `ser` (un built-in pisado que la serializacion llama).
            (&r, "Math.min = function () { return z; };\n__difuzzObserve([\"a\", [1]]);\nlet z = 1;\n".to_string()),
        ];
        for (run, js) in &casos {
            assert!(!puede_atrapar(js), "{js}");
            let (a, b) = (run.run(js), run.run(&format!("{js}/* try */\n")));
            assert_eq!((&a.stdout, &a.status, &a.flags, viol(&a)), (&b.stdout, &b.status, &b.flags, viol(&b)), "{js}\n{}\n{}", a.ctl, b.ctl);
        }
        // El de Symbol atrapado por invocar: sin el sensor no tendria flag.
        let e = r.run(&casos[0].1);
        assert!(e.flags.iter().any(|f| f == "coercion_symbol"), "{:?} {}", e.flags, e.ctl);
        // Paso 7 (I2): el de `ser` (antes, perezoso: sin flag ni `viol`).
        let e = r.run(&casos[casos.len() - 1].1);
        assert!(e.flags.iter().any(|f| f == "tdz_atrapada"), "{:?} {}", e.flags, e.ctl);
        // Paso 7 (I2, s04 sin palabras de la lista): la TDZ adentro de `onThrow` (String(e) de
        // un objeto con toString propio). La violacion es la misma (MEDIDO); prendido desde el
        // arranque ademas sale `stack` y la linea `viol` repetida (la TDZ pasa dos veces).
        let js = "const o = { [\"toStr\" + \"ing\"]() { return z; } };\nconsole.log(\"antes\");\nthrow o;\nlet z = 1;\n";
        assert!(!puede_atrapar(js));
        let (a, b) = (r.run(js), r.run(&format!("{js}/* try */\n")));
        assert_eq!((&a.stdout, &a.status), (&b.stdout, &b.status));
        assert_eq!(crate::recheck::violaciones(&a), crate::recheck::violaciones(&b), "{}\n{}", a.ctl, b.ctl);
        assert!(a.flags.iter().any(|f| f == "tdz_atrapada") && b.flags.iter().any(|f| f == "tdz_atrapada"), "{}\n{}", a.ctl, b.ctl);
        // Lo unico que cambia (MEDIDO): el top-level de un modulo ES que lanza. V8 pausa
        // (lo atrapa el import) y al formatear el stack prendia `stack`; la violacion
        // la saca onThrow igual.
        let js = "x;\nexport let x;\n";
        let (a, b) = (m.run(js), m.run(&format!("{js}/* try */\n")));
        assert_eq!((a.flags.clone(), viol(&a)), (vec!["tdz".to_string()], vec!["viol tdz 0:0".to_string()]), "{}", a.ctl);
        assert_eq!((b.flags.clone(), viol(&b)), (vec!["stack".to_string(), "tdz".to_string()], vec!["viol tdz 0:0".to_string()]), "{}", b.ctl);
    }

    /// b7-8: el programa del bug de V8 (`f1-bucle.js` de b7-8, 10-oct: con el
    /// sensor, SIGSEGV 20/20; sin el, 0/10; MEDIDO con node 24.14.1). El runner con el
    /// sensor omitido no prende el inspector: termina solo, sin `sensor_caido`, con
    /// `sensor_omitido` en el canal, y la cache de A lo guarda aparte. Que con el
    /// sensor muera no se afirma (depende del layout del heap): solo se informa.
    #[test]
    fn sensor_omitido_por_riesgo() {
        use super::{Preload, Runner, Status, SENSOR_CAIDO, SENSOR_OMITIDO};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("omitido").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
        let o = r.sin_sensor_por_riesgo();
        assert!(!o.con_sensor() && o.sensor_omitido() && !r.sensor_omitido());
        assert!(o.observing().sensor_omitido() && o.sin_sensor().sensor_omitido());
        assert_eq!((r.cache_tag(), o.cache_tag()), ("", SENSOR_OMITIDO));
        assert_eq!((r.observing().cache_tag(), o.observing().cache_tag()), ("observar", "observar sensor_omitido"));
        let js = "var v;\nvar basura = [];\nfor (var i = 0; i < 200; i++) {\n  basura.push(new Array(1000).fill({ i }));\n  try { class X extends 1 {} } catch {}\n}\nif (false) eval(\"\");\ntry { z; } catch {}\nlet z;\nconsole.log(41);\n";
        let e = o.run(js);
        assert_eq!((&e.status, e.stdout.as_str()), (&Status::Exited(0), "41\n"), "{}", e.ctl);
        assert!(e.ctl.starts_with("sensor_omitido\n") && !e.ctl.lines().any(|l| l == SENSOR_CAIDO), "{}", e.ctl);
        assert_eq!(e.sensor_degradado(), Some(SENSOR_OMITIDO));
        // Sin el sensor no hay violaciones (la TDZ atrapada no se ve): es el costo.
        assert!(!e.ctl.lines().any(|l| l.starts_with("viol ")), "{}", e.ctl);
        // Integracion 7.8 + 7.1/7.4 (camino de B tras I3): con el runner omitido, la corrida con
        // cobertura y el tope mayor tampoco prende el sensor, y el camino de B no sale de ahi.
        let cov = o.run_con_cobertura(js, &[], &[("DIFUZZ_MAX_PAUSAS", super::MAX_PAUSAS_MAYOR)]);
        assert_eq!((cov.sensor_degradado(), cov.cobertura().is_some()), (Some(SENSOR_OMITIDO), true), "{}", cov.ctl);
        assert!(!cov.ctl.lines().any(|l| l.starts_with("viol ")), "{}", cov.ctl);
        let camino = crate::recheck::camino_sensor_i3(&o, &o, js, &e, None);
        assert!(matches!(camino, crate::recheck::Camino::NoSe(ref n) if n.contains(SENSOR_OMITIDO)), "{camino:?}");
        let c = r.run(js);
        eprintln!("b7-8: con el sensor: {:?}, sensor_caido={}", c.status, c.ctl.lines().any(|l| l == SENSOR_CAIDO));
    }

    /// 11.5 (I3): una ejecucion que pasa el tope de RSS no devuelve un `Exec`: desenrolla con
    /// `Excedido` (causa, mensaje y el programa). La cortan antes de reservar lo que pide (3 GiB en
    /// trozos de 32 MiB, tocados); con el sensor prendido o sin el, igual. Un programa chico, con el
    /// mismo tope, corre como siempre.
    #[test]
    fn tope_de_rss_desenrolla_con_excedido() {
        use super::{Excedido, Preload, Runner, Status, NODE_MEMORIA};
        use std::time::Duration;
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("tope-rss").unwrap();
        let tope = 256u64 << 20;
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(60)).con_tope_rss(tope);
        assert_eq!(r.run("console.log(1);\n").status, Status::Exited(0));
        let js = "const b = [];\nfor (let i = 0; i < 96; i++) b.push(Buffer.alloc(32 << 20, 1));\nconsole.log(b.length);\n";
        for runner in [r.sin_sensor(), r.sensor_entero()] {
            let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runner.run(js))) else {
                panic!("tenia que pasar el tope");
            };
            let x = p.downcast::<Excedido>().expect("el payload es Excedido");
            assert_eq!((x.causa, x.programa.as_str(), x.ext), (NODE_MEMORIA, js, "cjs"));
            assert!(x.mensaje.starts_with("node_memoria: ") && x.mensaje.contains("> 256 MiB (matada al pasarlo)"), "{}", x.mensaje);
            // El pico que vio difuzz: lejos de los 3 GiB pedidos (la mato a tiempo).
            let pico: u64 = x.mensaje.split("RSS de node: ").nth(1).and_then(|s| s.split(' ').next()).and_then(|n| n.parse().ok()).unwrap();
            assert!(pico < 1024, "pico {pico} MiB: {}", x.mensaje);
        }
    }

    /// 11.7 (I5): una ejecucion que escribe mas que el tope de salida (sumando stdout y stderr) no
    /// devuelve un `Exec`: desenrolla con `Excedido` (`NODE_SALIDA`) y node muere (un programa que
    /// escribiria 1 GiB no llega a escribirlo). Justo en el tope, se compara como siempre.
    #[test]
    fn tope_de_salida_desenrolla_con_excedido() {
        use super::{Excedido, Preload, Runner, Status, NODE_SALIDA};
        use std::time::{Duration, Instant};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("tope-salida").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(60)).con_tope_salida(1 << 20).sin_sensor();
        // Exactamente 1 MiB (512 KiB por cada pipe): entra.
        let justo = "const s = \"x\".repeat(512 * 1024 - 1) + \"\\n\";\nprocess.stdout.write(s);\nprocess.stderr.write(s);\n";
        let e = r.run(justo);
        assert_eq!((e.status, e.stdout.len(), e.stderr.len()), (Status::Exited(0), 512 * 1024, 512 * 1024));
        for (que, js) in [
            ("stdout", "const s = \"x\".repeat(1 << 20) + \"\\n\";\nfor (let i = 0; i < 1024; i++) process.stdout.write(s);\n"),
            ("stdout y stderr", "const s = \"x\".repeat(700 * 1024) + \"\\n\";\nprocess.stdout.write(s);\nprocess.stderr.write(s);\n"),
            // `fs.writeSync`: `process.stderr.write` de 2 MiB a un pipe deja la mayor parte en la cola
            // de node, que el bucle no deja vaciar (MEDIDO: timeout, sin pasar el tope).
            ("stderr, mientras gira", "const fs = require(\"fs\");\nfor (let i = 0; i < 32; i++) fs.writeSync(2, \"x\".repeat(64 * 1024));\nwhile (true) {}\n"),
        ] {
            let t0 = Instant::now();
            let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.run(js))) else {
                panic!("{que}: tenia que pasar el tope");
            };
            let x = p.downcast::<Excedido>().expect("el payload es Excedido");
            assert_eq!((x.causa, x.programa.as_str()), (NODE_SALIDA, js), "{que}");
            assert!(x.mensaje.starts_with("node_salida: ") && x.mensaje.contains("paso el tope de salida de 1 MiB"), "{que}: {}", x.mensaje);
            // Cortado al pasarlo, no al terminar (1 GiB, o el bucle, que solo corta el timeout).
            assert!(t0.elapsed() < Duration::from_secs(30), "{que}: {:?}", t0.elapsed());
        }
    }

    /// 11.6 (I4): node lanzado como en `run_file` (con `setpriv` y con el prctl en pre_exec) muere
    /// con SIGKILL cuando termina el HILO que lo lanzo, aunque el proceso siga: `PR_SET_PDEATHSIG` es
    /// por hilo (MEDIDO aca). Lanzado desde un hilo que sigue vivo, sigue vivo. En difuzz el hilo es
    /// el worker, que espera a node: solo lo pierde si muere el proceso entero.
    #[test]
    fn pdeathsig_sigue_al_hilo() {
        use std::time::{Duration, Instant};
        if !crate::test_util::requiere_node() {
            return;
        }
        let lanzar = |setpriv: Option<&std::path::Path>| {
            let (mut c, pre) = super::comando_node(setpriv);
            if pre {
                super::prctl_en_el_hijo(&mut c);
            }
            c.args(["-e", "setInterval(() => {}, 1000);"]).stdin(std::process::Stdio::null());
            // Lo cosecha este test con waitpid (abajo).
            #[allow(clippy::zombie_processes)]
            let h = c.spawn().unwrap();
            // Hasta que el exec de node ya paso (con setpriv: despues de su prctl). Antes, si el
            // hilo termina, la senal todavia no estaba puesta: la ventana de `morir_con_el_padre`.
            let node = std::fs::canonicalize(super::node_path()).unwrap();
            let t0 = Instant::now();
            while std::fs::read_link(format!("/proc/{}/exe", h.id())).ok() != Some(node.clone()) {
                assert!(t0.elapsed() < Duration::from_secs(10), "node no arranco");
                std::thread::sleep(Duration::from_millis(5));
            }
            h.id() as libc::pid_t
        };
        // (cosechado, termino por SIGKILL)
        let esperar = |pid: libc::pid_t, hasta: Duration| {
            let t0 = Instant::now();
            loop {
                let mut st = 0;
                // SAFETY: waitpid de un hijo propio, puntero a la pila.
                if unsafe { libc::waitpid(pid, &mut st, libc::WNOHANG) } == pid {
                    return (true, libc::WIFSIGNALED(st) && libc::WTERMSIG(st) == libc::SIGKILL);
                }
                if t0.elapsed() > hasta {
                    return (false, false);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let modos: Vec<Option<&std::path::Path>> = super::setpriv().into_iter().map(Some).chain([None]).collect();
        eprintln!("11.6: setpriv = {:?}", super::setpriv());
        for setpriv in modos {
            // Desde un hilo que termina enseguida: muere con el.
            let pid = std::thread::scope(|s| s.spawn(|| lanzar(setpriv)).join().unwrap());
            assert_eq!(esperar(pid, Duration::from_secs(10)), (true, true), "setpriv {setpriv:?}: node {pid} sobrevivio a su hilo");
            // Desde este hilo (vivo): sigue vivo hasta que se lo mata.
            let pid = lanzar(setpriv);
            assert_eq!(esperar(pid, Duration::from_millis(500)), (false, false), "setpriv {setpriv:?}: murio sin que muriera su hilo");
            // SAFETY: kill a un hijo propio sin cosechar.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            assert_eq!(esperar(pid, Duration::from_secs(10)), (true, true));
        }
    }

    /// 11.6 (I4): un node en curso esta en `VIVOS` (lo que recorre `matar_vivos`, la segunda senal)
    /// con el pid de su grupo, y al terminar ya no. Matar ese grupo corta la ejecucion por senal, no
    /// por el timeout. No llama a `matar_vivos`: mataria los node de los otros tests del proceso (eso
    /// lo prueba `salida_celdas::segunda_senal_y_sigkill_no_dejan_node`, con el binario).
    #[test]
    fn node_en_curso_esta_en_vivos() {
        use super::{Preload, Runner, Status};
        use std::time::{Duration, Instant};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("vivos").unwrap();
        // Sin el sensor: con el, una senal se repite sin el (`run_inner`) y la repeticion gira hasta
        // el timeout. Tras la segunda senal difuzz sale y no repite nada.
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(60)).with_sensor(false);
        let cwd = r.cwd.clone();
        let registrados = || super::VIVOS.iter().map(|v| v.load(std::sync::atomic::Ordering::SeqCst)).filter(|p| *p > 0).collect::<Vec<i32>>();
        // El node de este runner: el registrado cuyo cwd es el del runner (los otros tests usan otros).
        let mio = || registrados().into_iter().find(|p| std::fs::read_link(format!("/proc/{p}/cwd")).is_ok_and(|d| d == cwd));
        let (e, t, pid) = std::thread::scope(|s| {
            let h = s.spawn(|| {
                let t0 = Instant::now();
                let e = r.run("while (true) {}\n");
                (e, t0.elapsed())
            });
            let t0 = Instant::now();
            let pid = loop {
                if let Some(p) = mio() {
                    break p;
                }
                assert!(t0.elapsed() < Duration::from_secs(20), "el node del runner no aparecio en VIVOS");
                std::thread::sleep(Duration::from_millis(20));
            };
            // Lo que hace `matar_vivos` con cada uno, solo con este.
            // SAFETY: kill(2) al grupo de un node en curso de este test.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            let (e, t) = h.join().unwrap();
            (e, t, pid)
        });
        assert_eq!(e.status, Status::Signal, "{}", e.stderr);
        assert!(t < Duration::from_secs(30), "{t:?}");
        assert!(!registrados().contains(&pid), "sigue registrado despues de cosecharlo");
    }

    /// 11.5 (I3): `Cosecha` da el pico de RSS de node (`ru_maxrss` de `wait4`), tambien el que no
    /// alcanzo a verse mientras corria.
    #[test]
    // Lo cosecha `Cosecha` con wait4, no `Child::wait` (como en `run_file`).
    #[allow(clippy::zombie_processes)]
    fn cosecha_da_el_pico_de_rss() {
        if !crate::test_util::requiere_node() {
            return;
        }
        let hijo = std::process::Command::new(super::node_path())
            .args(["-e", "Buffer.alloc(200 << 20, 1);"])
            .stdin(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut c = super::Cosecha::new(hijo.id());
        assert!(c.esperar(true).unwrap());
        assert_eq!(c.status(), super::Status::Exited(0));
        let pico = c.pico_rss() >> 20;
        assert!((200..1024).contains(&pico), "pico {pico} MiB");
        // Ya cosechado: ni espera ni mata otra vez.
        assert!(c.esperar(false).unwrap());
        c.matar();
    }
}
