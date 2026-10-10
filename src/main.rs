//! difuzz: fuzzer diferencial de minifiers de JavaScript (original y minificado
//! bajo node; descartes, excepciones, sensor de asunciones, canales, lint).
//!
//! GEN (corpus) → MIN (minify, backend swc|oxc) → EXEC (runner) → CMP (oracle).
//! Mutaciones opcionales (--mutaciones), sin reduccion. Feo a proposito: el
//! pipeline es la entrega.

mod analisis;
mod backend;
mod binario;
mod cache;
mod checks;
mod clasificar;
mod corpus;
mod corrida;
#[cfg(test)]
mod e2e_tests;
mod emitir;
mod huerfanos;
mod indice;
#[cfg(test)]
mod invocar_tests;
mod known;
mod minify;
mod mutate;
mod oracle;
#[cfg(test)]
mod oraculo_tests;
mod recheck;
mod runner;
#[cfg(test)]
mod sandbox_tests;
mod stubs;
mod subproceso;
#[cfg(test)]
mod test_util;
mod triage;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use runner::Status;

const USAGE: &str = "\
uso: difuzz <corpus_dir> [--out DIR] [--minifier M] [--config C] [--forma F] [--carril K] [--timeout-ms N] [--limit N] [--jobs N] [--no-observe]
             [--invocar] [--mutaciones N] [--mutaciones-extra N] [--stubs] [--cache DIR | --no-cache] [--indice DIR] [--en-proceso]
             [--mem-hijo MiB] [--sandbox | --sandbox-red]
       difuzz --cruce DIR
       difuzz --emitir ARCHIVO [--minifier M] [--config C] [--forma F] [--carril K]
       difuzz --help

  corpus_dir     raiz con archivos input.js (p. ej. crates/swc_ecma_minifier/tests)
  --out DIR      donde guardar hallazgos, en DIR/<celda>/ (default: ./difuzz-out)
                 R1-H09: celdas distintas no se pisan; DIR/<celda>/CORRIDA.txt dice si la corrida termino
  --minifier M   swc | oxc                          (default: oxc)
  --config C     carril_a | core_defaults           (default: carril_a)
  --forma F      tal_cual | en_funcion              (default: tal_cual)
  --carril K     a (script, .cjs) | b (modulo ES, .mjs) (default: a)
  --timeout-ms N timeout por ejecucion de node    (default: 5000)
  --limit N      procesar solo los primeros N casos (N >= 1)
  --jobs N       workers en paralelo                (default: nucleos disponibles)
  --no-observe   no agregar el epilogo observador a programas que no imprimen
  --invocar      el epilogo observador ademas llama a las funciones observadas
  --mutaciones N variantes por caso (borrar sentencias, perturbar literales), solo
                 por la pasada cruda + lint                (default: 0)
  --mutaciones-extra N  M33: ademas, N variantes de los operadores nuevos (literal a
                 negativo, envolver, duplicar, if, var/let/const, \"use strict\"),
                 numeradas despues de las clasicas         (default: 0)
  --stubs        M30: un import/require que no resuelve pasa a un modulo stub inerte
                 (igual en A y B) en vez de descartar el caso como enlace (default: off)
  --sandbox      node corre con su modelo de permisos (--permission de node 24): lee solo el
                 preload, el programa y su directorio de trabajo, escribe solo en ese directorio,
                 sin procesos hijos, workers, addons ni WASI. A y B igual; si el original toca
                 un permiso negado, el caso va a descartes.tsv con causa sandbox (default: off).
                 NO frena la red ni los sockets unix: no es una barrera contra codigo hostil
  --sandbox-red  --sandbox mas namespaces de Linux sin root (usuario, red, montajes): sin red
                 (solo un loopback propio), /run tapado y los sockets unix del sistema tapados.
                 Solo Linux con user namespaces; si no se puede armar, sale con 1 (default: off)
  --cache DIR    cache de la ejecucion del original, compartible entre celdas (default: <out>/cache-a)
  --no-cache     no leer ni escribir la cache
                 (cache_a_hits no es determinista: con la cache en frio depende de --jobs y del
                 orden en que los workers toman los casos; los resultados no cambian)
  --indice DIR   anotar cada hallazgo en DIR/index-<celda>.tsv (directorio compartido por las celdas)
  --cruce DIR    imprimir la tabla de cruce de DIR/index-*.tsv, sin correr nada
  --emitir ARCH  imprimir el original y el minificado de ARCH (sin correr node)
  --en-proceso   minificar en el proceso de difuzz. Por defecto se minifica en un hijo
                 persistente por worker: un abort, un desborde de pila o un cuelgue del
                 minifier queda en panic/ (con la senal) y la corrida sigue. Un minifier que no
                 termina en 60 s o que pasa el presupuesto de memoria va a recursos/ (causa
                 timeout o memoria en meta.txt; RECURSOS= en las stats), no a panic/
  --mem-hijo MiB presupuesto de RSS de los hijos de cada worker: 3/4 para sus minificadores,
                 sumados, y 1/4 para su hijo de analisis (default: MemTotal/2 / workers, entre
                 256 MiB y 4 GiB). El hijo de analisis que se pasa es un fallo del analisis, no recursos/

  oraculo sin debug assertions (R1-H05): este binario lleva debug assertions (Cargo.toml) y el
  paquete npm del minifier no. Ante un panic, el caso se re-minifica con el mismo difuzz compilado
  sin ellas; si ese minifica, el caso se compara con su minificado (div/ con la nota
  solo_debug_assertions, o asserts/ si no diverge); si tambien falla, va a panic/.
  Se compila con:  cargo build --release && cargo build --profile oraculo
  y se busca en <este binario>-oraculo y <dir>/../oraculo/difuzz, o solo en DIFUZZ_ORACULO=RUTA si
  esta (DIFUZZ_ORACULO=ninguno lo apaga). Si no hay, un aviso al arrancar y los panics van a
  panic/ como siempre
  --help, -h     imprimir esta ayuda (stdout) y salir con 0, en cualquier posicion menos como valor
                 de otra opcion (`--out -h` es el directorio -h)

  salida (R2-H06): cada caso termina en un destino, div/ known/ panic/ recursos/ (un directorio por
  caso), en iguales (sin archivo) o con una fila en descartes.tsv (caso, causa, elapsed_ms), incluidos
  mudo_sin_bindings, ilegible y panic_del_fuzzer. lint/ y asserts/ son ortogonales (el caso cuenta
  tambien donde termino). Sin --mutaciones (las variantes caso__mutK tambien dejan filas):
    total = iguales + DIVERGENCIAS + conocidas + PANICS + RECURSOS + filas(descartes.tsv)
  (un panic del fuzzer despues de contar el caso lo cuenta dos veces: panics_del_fuzzer= lo dice)

  codigos de salida (R2-H09):
    0    corrida entera (con o sin hallazgos; CORRIDA.txt: completa); --cruce, --emitir y --help sin error
    1    error de entrada o de entorno: corpus inexistente, que no es directorio o sin input.js; --out
         que no se puede escribir; escrituras perdidas o casos sin contar (CORRIDA.txt: incompleta);
         el oraculo reescrito en el lugar a mitad de corrida (R1-H18/M3; CORRIDA.txt: incompleta, con
         binario_cambiado; con /proc, reemplazar difuzz o el oraculo por otro archivo no corta la
         corrida: se siguen lanzando los del arranque; sin /proc se vigilan por ruta y cualquier
         cambio corta); --cruce sin indices; --emitir que no puede leer o minificar
    2    error de uso: opcion desconocida, valor o combinacion invalida (con este texto en stderr)
    130  interrumpida por SIGINT o SIGTERM (CORRIDA.txt: interrumpida)";

/// R2-H09: los codigos de salida de la tabla del USAGE (0 es el exito).
const SALIDA_ERROR: i32 = 1;
const SALIDA_USO: i32 = 2;
const SALIDA_INTERRUMPIDA: i32 = 130;

/// Las opciones de `Cli::parse` que toman el argumento siguiente como valor. Las
/// usa `pide_ayuda`; el test `opciones_con_valor_al_dia` las cruza con `Cli::parse`.
const OPCIONES_CON_VALOR: [&str; 15] = [
    "--out",
    "--timeout-ms",
    "--limit",
    "--jobs",
    "--minifier",
    "--cache",
    "--indice",
    "--mem-hijo",
    "--emitir",
    "--cruce",
    "--mutaciones",
    "--mutaciones-extra",
    "--config",
    "--forma",
    "--carril",
];

/// R2-H09: `--help`/`-h` solo como argumento propio, no como el valor de una
/// opcion: `--out -h` es el directorio `-h` (antes pedia la ayuda y salia con 0).
fn pide_ayuda(args: &[String]) -> bool {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--help" || a == "-h" {
            return true;
        }
        if OPCIONES_CON_VALOR.contains(&a.as_str()) {
            it.next();
        }
    }
    false
}

/// Las opciones con valor repetidas (`--jobs 2 --jobs 4`): gana la ultima, como
/// antes, pero main lo avisa por stderr. Una entrada por opcion repetida, con el
/// valor que gana, en el orden de su primera aparicion. Recorre como `pide_ayuda`.
fn opciones_repetidas(args: &[String]) -> Vec<(String, String)> {
    let mut vistas: Vec<(&str, usize, &str)> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if !OPCIONES_CON_VALOR.contains(&a.as_str()) {
            continue;
        }
        let Some(v) = it.next() else { break };
        match vistas.iter_mut().find(|(o, ..)| *o == a) {
            Some(e) => {
                e.1 += 1;
                e.2 = v;
            }
            None => vistas.push((a, 1, v)),
        }
    }
    vistas.into_iter().filter(|(_, n, _)| *n > 1).map(|(o, _, v)| (o.to_string(), v.to_string())).collect()
}

struct Cli {
    corpus: PathBuf,
    out: PathBuf,
    timeout: Duration,
    limit: Option<usize>,
    jobs: usize,
    observe: bool,
    minifier: backend::Minifier,
    preset: minify::Preset,
    forma: minify::Forma,
    /// `None` con `--no-cache`.
    cache: Option<PathBuf>,
    indice: Option<PathBuf>,
    /// Mejora 7: el epilogo ademas llama a las funciones observadas.
    invocar: bool,
    /// Mejora 10: variantes por caso (mutate.rs).
    mutaciones: usize,
    /// M33: variantes de los operadores nuevos, despues de las clasicas.
    mutaciones_extra: usize,
    /// M30: stubs para los specifiers que no resuelven (stubs.rs).
    stubs: bool,
    /// n-sandbox: `--sandbox` / `--sandbox-red` (runner::Sandbox). Por defecto apagado.
    sandbox: runner::Sandbox,
    kind: minify::SourceKind,
    /// M24: `--cruce DIR`: solo imprimir la tabla de cruce, sin correr nada.
    cruce: Option<PathBuf>,
    /// M34: minificar en un hijo (subproceso.rs). Por defecto; `--en-proceso` lo apaga.
    subproceso: bool,
    /// M29: `--emitir ARCHIVO`: imprimir original y minificado, sin correr nada.
    emitir: Option<PathBuf>,
    /// R2-H04: `--mem-hijo MiB`, presupuesto de RSS de los hijos de un worker.
    mem_hijo: Option<u64>,
}

impl Cli {
    /// Celda de la matriz: nombra la salida y el oraculo (known-sintomas). El
    /// backend va primero: sintomas de un minifier no aplican al otro.
    fn cell(&self) -> String {
        let mut cell = format!("{}+{}+{}", self.minifier.name(), self.preset.name(), self.forma.name());
        // Carril B al final; el carril A conserva el nombre de siempre.
        if self.kind == minify::SourceKind::Module {
            cell.push_str("+carril_b");
        }
        if self.invocar {
            cell.push_str("+invocar");
        }
        if self.mutaciones > 0 {
            cell.push_str(&format!("+mut{}", self.mutaciones));
        }
        if self.mutaciones_extra > 0 {
            cell.push_str(&format!("+mutx{}", self.mutaciones_extra));
        }
        if self.stubs {
            cell.push_str("+stubs");
        }
        // n-sandbox: cambia lo que se compara (lo que toca el sandbox va a descartes).
        match self.sandbox {
            runner::Sandbox::Apagado => {}
            runner::Sandbox::Permisos => cell.push_str("+sandbox"),
            runner::Sandbox::PermisosYRed => cell.push_str("+sandbox_red"),
        }
        cell
    }
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Cli, String> {
        let mut corpus = None;
        let mut out = PathBuf::from("difuzz-out");
        let mut timeout = Duration::from_millis(5000);
        let mut limit = None;
        let mut observe = true;
        let mut minifier = backend::Minifier::POR_DEFECTO;
        let mut preset = minify::Preset::CarrilA;
        let mut forma = minify::Forma::TalCual;
        let mut kind = minify::SourceKind::Script;
        let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
        let mut cache = None;
        let mut no_cache = false;
        let mut indice = None;
        let mut invocar = false;
        let mut mutaciones = 0;
        let mut mutaciones_extra = 0;
        let mut stubs = false;
        let mut sandbox = runner::Sandbox::Apagado;
        let mut cruce = None;
        let mut subproceso = true;
        let mut emitir = None;
        let mut mem_hijo = None;

        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--out" => out = PathBuf::from(it.next().ok_or("--out necesita un valor")?),
                "--timeout-ms" => {
                    let v = it.next().ok_or("--timeout-ms necesita un valor")?;
                    timeout = Duration::from_millis(
                        v.parse().map_err(|_| format!("--timeout-ms invalido: {v}"))?,
                    );
                    // M27 (b): 0 haria de todo un timeout.
                    if timeout.is_zero() {
                        return Err("--timeout-ms tiene que ser >= 1".into());
                    }
                }
                "--limit" => {
                    let v = it.next().ok_or("--limit necesita un valor")?;
                    limit = Some(v.parse().map_err(|_| format!("--limit invalido: {v}"))?);
                    // R2-H09: como --jobs 0, error de uso (antes salia con 1 despues
                    // de borrar la salida de la celda y dejarla incompleta).
                    if limit == Some(0) {
                        return Err("--limit tiene que ser >= 1".into());
                    }
                }
                "--jobs" => {
                    let v = it.next().ok_or("--jobs necesita un valor")?;
                    jobs = v.parse().map_err(|_| format!("--jobs invalido: {v}"))?;
                    if jobs == 0 {
                        return Err("--jobs tiene que ser >= 1".into());
                    }
                }
                "--no-observe" => observe = false,
                "--minifier" => {
                    let v = it.next().ok_or("--minifier necesita un valor")?;
                    minifier = backend::Minifier::parse(&v)
                        .ok_or_else(|| format!("--minifier invalido: {v} ({})", backend::Minifier::ALL))?;
                }
                "--cache" => cache = Some(PathBuf::from(it.next().ok_or("--cache necesita un valor")?)),
                "--no-cache" => no_cache = true,
                "--indice" => indice = Some(PathBuf::from(it.next().ok_or("--indice necesita un valor")?)),
                "--invocar" => invocar = true,
                "--en-proceso" => subproceso = false,
                "--mem-hijo" => {
                    let v = it.next().ok_or("--mem-hijo necesita un valor")?;
                    let m: u64 = v.parse().map_err(|_| format!("--mem-hijo invalido: {v}"))?;
                    if m == 0 {
                        return Err("--mem-hijo tiene que ser >= 1".into());
                    }
                    mem_hijo = Some(m);
                }
                "--emitir" => emitir = Some(PathBuf::from(it.next().ok_or("--emitir necesita un archivo")?)),
                "--cruce" => cruce = Some(PathBuf::from(it.next().ok_or("--cruce necesita un directorio")?)),
                "--mutaciones" => {
                    let v = it.next().ok_or("--mutaciones necesita un valor")?;
                    mutaciones = v.parse().map_err(|_| format!("--mutaciones invalido: {v}"))?;
                }
                "--mutaciones-extra" => {
                    let v = it.next().ok_or("--mutaciones-extra necesita un valor")?;
                    mutaciones_extra = v.parse().map_err(|_| format!("--mutaciones-extra invalido: {v}"))?;
                }
                "--stubs" => stubs = true,
                // n-sandbox: --sandbox-red incluye --sandbox (en cualquier orden).
                "--sandbox" => sandbox = sandbox.max(runner::Sandbox::Permisos),
                "--sandbox-red" => sandbox = runner::Sandbox::PermisosYRed,
                "--config" => {
                    let v = it.next().ok_or("--config necesita un valor")?;
                    preset = minify::Preset::parse(&v)
                        .ok_or_else(|| format!("--config invalido: {v} ({})", minify::Preset::ALL))?;
                }
                "--forma" => {
                    let v = it.next().ok_or("--forma necesita un valor")?;
                    forma = minify::Forma::parse(&v)
                        .ok_or_else(|| format!("--forma invalida: {v} ({})", minify::Forma::ALL))?;
                }
                "--carril" => {
                    let v = it.next().ok_or("--carril necesita un valor")?;
                    kind = minify::SourceKind::parse(&v)
                        .ok_or_else(|| format!("--carril invalido: {v} ({})", minify::SourceKind::ALL))?;
                }
                other if other.starts_with("--") => return Err(format!("opcion desconocida: {other}")),
                // M27 (c): antes ganaba el ultimo en silencio.
                other if corpus.is_some() => return Err(format!("un solo <corpus_dir>: sobra {other}")),
                other => corpus = Some(PathBuf::from(other)),
            }
        }

        // M24: --cruce solo lee indices. Las demas opciones se aceptan y no
        // cambian nada; un corpus no tiene sentido.
        if let Some(dir) = cruce {
            if corpus.is_some() {
                return Err("--cruce no corre nada: sin <corpus_dir>".into());
            }
            corpus = Some(PathBuf::new());
            cruce = Some(dir);
        }
        // M29: --emitir toma un archivo, no un corpus.
        if emitir.is_some() {
            if corpus.is_some() || cruce.is_some() {
                return Err("--emitir no corre nada: sin <corpus_dir> ni --cruce".into());
            }
            corpus = Some(PathBuf::new());
        }
        if no_cache && cache.is_some() {
            return Err("--cache y --no-cache se excluyen".into());
        }
        let cache = (!no_cache).then(|| cache.unwrap_or_else(|| out.join("cache-a")));
        if invocar && !observe {
            return Err("--invocar necesita el observador (no va con --no-observe)".into());
        }
        Ok(Cli {
            corpus: corpus.ok_or("falta <corpus_dir>")?,
            out,
            timeout,
            limit,
            jobs,
            observe,
            minifier,
            preset,
            forma,
            cache,
            indice,
            invocar,
            mutaciones,
            mutaciones_extra,
            stubs,
            sandbox,
            kind,
            cruce,
            subproceso,
            emitir,
            mem_hijo,
        })
    }
}

#[derive(Default)]
struct Stats {
    total: usize,
    unreadable: usize,
    /// Descartes por el original, por causa. `original=` en la linea de stats
    /// es la suma, igual que antes del desglose.
    a_exit: usize,
    a_timeout: usize,
    /// Node no acepta el original como script (`node --check`).
    a_invalid: usize,
    a_signal: usize,
    /// Mudo: el original con epilogo no termino con salida.
    observed_not_ok: usize,
    /// Mudo con `--no-observe`.
    silent_no_observe: usize,
    parse_esm: usize,
    parse_other: usize,
    codegen_error: usize,
    /// Fallas del runner (error de difuzz, no del programa): A + B.
    runner_failed: usize,
    /// M32: las de A (antes de comparar). Las de B son `runner_failed - runner_a`.
    runner_a: usize,
    panics: usize,
    /// R1-H08 / R2-H04: el minifier no termino a tiempo o paso el presupuesto
    /// de memoria (`recursos/`). No cuentan en `panics`.
    recursos_timeout: usize,
    recursos_memoria: usize,
    /// Sospechosos del lint de escrituras sobre literales (independiente del veredicto).
    lint: usize,
    same: usize,
    divergences: usize,
    /// Divergencias esperadas: asuncion documentada o backlog de SWC (known.rs).
    known: usize,
    /// Conocidas que divergieron con otro sintoma: cuentan en `divergences`.
    known_drift: usize,
    /// Casos mudos comparados via epilogo (subconjunto de same + divergences).
    observed: usize,
    /// Casos mudos sin bindings top-level que observar.
    silent_unobservable: usize,
    /// Mudos sin bindings pero con globals creados: los comparo la pasada cruda
    /// (el preload los imprime al salir). Subconjunto de `same`.
    observed_globals: usize,
    /// Comparados cuyo original lanzo (subconjunto de same + divergences).
    threw: usize,
    /// Panics del propio fuzzer (no del minifier: esos van en `panics`). Un
    /// caso perdido, no un worker perdido.
    fuzzer_panics: usize,
    /// El minificado es identico al original: B no se corre, cuenta en `same`.
    identical: usize,
    /// Sospechosos del lint ya triageados como equivalentes (lint-triage.tsv):
    /// no se reportan ni cuentan en `lint`.
    lint_triaged: usize,
    /// Ejecuciones de A que salieron de la cache (cache.rs).
    cache_hits: usize,
    /// Mejora 10: las variantes van aparte, no suman en las cifras del corpus.
    mutaciones: MutStats,
    /// Carril B: no parsea como modulo pero si como script (espejo de `parse_esm`).
    parse_script: usize,
    /// El original no enlaza (import/require que no resuelve; runner::LINK_EXIT
    /// con `link` en el canal de control). Los dos carriles desde M9.
    a_link: usize,
    /// M3: divergencia que no se confirma: A corrido otra vez en fresco da otra
    /// salida. No se reporta; cuenta dentro de `original=`.
    a_nondet: usize,
    /// M18: kind de la ultima divergencia (reportada o conocida) del caso; la
    /// usa process_mutants para el dedup. No se suma en `merge`.
    last_div: Option<oracle::Kind>,
    /// M18: en las variantes, el kind con que ya divergio el base: una variante
    /// con el mismo kind no se reporta (`inherited`). No se suma en `merge`.
    skip_kind: Option<oracle::Kind>,
    inherited: usize,
    /// M31 (a): observaciones con `--invocar` que no terminaron. R2-H01: en el
    /// epilogo, la pasada de invocacion (adicional) sin salida: vale la normal;
    /// en el envoltorio de modulos, la que se repitio sin invocar. Va en una
    /// linea aparte (solo con `--invocar`).
    invocar_retry: usize,
    /// I3/M1 (revision-b456): comparaciones donde A o B no corrieron enteros con
    /// el sensor de violaciones y los dos se repitieron sin el. Ortogonal, como LINT.
    sensor_repetidos: usize,
    /// R1-H05: casos cuyo minificado salio del oraculo sin debug assertions (el
    /// binario con asserts hizo panic). Ortogonal, como LINT: el caso cuenta
    /// tambien donde termino (iguales, DIVERGENCIAS, conocidas, descartes).
    solo_asserts: usize,
    /// R1-H05: de esos, los que no terminaron en div/ ni known/ (van a asserts/).
    solo_asserts_sin_div: usize,
    /// A6/R1-H12: casos donde fallo el analisis de difuzz (no el minifier):
    /// panic o abort del parser de SWC, de los chequeos o de las mutaciones.
    /// Ortogonal, como LINT. Notas en meta.txt (analisis::notas).
    fallos_analisis: usize,
    /// A6: de esos, los descartados con causa `analisis` (el epilogo de un mudo).
    descartes_analisis: usize,
    /// n-sandbox: casos descartados porque el original toco el sandbox (causa `sandbox`).
    /// En una linea aparte (solo con `--sandbox`): no cambia la de `total=`.
    a_sandbox: usize,
    /// R1-H13: casos (pasada cruda) cuyo analisis con el parser de SWC necesito
    /// errores recuperados u otro carril, sin perder ningun texto.
    analisis_recuperado: usize,
    /// R1-H13: casos con algun texto que SWC no parsea (sin flags o sin lint).
    analisis_perdido: usize,
    /// R1-H13: de esos, los que no corren el lint (falta el original o el minificado).
    lint_sin_analisis: usize,
    /// R1-H14: pasadas de bloques que se compararon (main::pasada_bloques).
    bloques: usize,
    /// R1-H14: de esas, las que no tenian epilogo normal (antes mudos_sin_bindings).
    bloques_unica: usize,
    /// R1-H14: pasadas de bloques cuyo original no termino con salida (no cuentan).
    bloques_sin_salida: usize,
    /// R1-H13/H14: nota de la pasada en curso para meta.txt (la lee
    /// compare_and_report). No se suma en `merge`.
    nota_pasada: Option<String>,
}

impl Stats {
    fn merge(&mut self, o: &Stats) {
        self.total += o.total;
        self.unreadable += o.unreadable;
        self.a_exit += o.a_exit;
        self.a_timeout += o.a_timeout;
        self.a_invalid += o.a_invalid;
        self.a_signal += o.a_signal;
        self.observed_not_ok += o.observed_not_ok;
        self.silent_no_observe += o.silent_no_observe;
        self.parse_esm += o.parse_esm;
        self.parse_other += o.parse_other;
        self.codegen_error += o.codegen_error;
        self.runner_failed += o.runner_failed;
        self.runner_a += o.runner_a;
        self.panics += o.panics;
        self.recursos_timeout += o.recursos_timeout;
        self.recursos_memoria += o.recursos_memoria;
        self.lint += o.lint;
        self.same += o.same;
        self.divergences += o.divergences;
        self.known += o.known;
        self.known_drift += o.known_drift;
        self.observed += o.observed;
        self.silent_unobservable += o.silent_unobservable;
        self.observed_globals += o.observed_globals;
        self.fuzzer_panics += o.fuzzer_panics;
        self.threw += o.threw;
        self.identical += o.identical;
        self.lint_triaged += o.lint_triaged;
        self.cache_hits += o.cache_hits;
        self.mutaciones.merge(&o.mutaciones);
        self.parse_script += o.parse_script;
        self.a_link += o.a_link;
        self.a_nondet += o.a_nondet;
        self.invocar_retry += o.invocar_retry;
        self.sensor_repetidos += o.sensor_repetidos;
        self.solo_asserts += o.solo_asserts;
        self.solo_asserts_sin_div += o.solo_asserts_sin_div;
        self.fallos_analisis += o.fallos_analisis;
        self.descartes_analisis += o.descartes_analisis;
        self.a_sandbox += o.a_sandbox;
        self.analisis_recuperado += o.analisis_recuperado;
        self.analisis_perdido += o.analisis_perdido;
        self.lint_sin_analisis += o.lint_sin_analisis;
        self.bloques += o.bloques;
        self.bloques_unica += o.bloques_unica;
        self.bloques_sin_salida += o.bloques_sin_salida;
    }

    /// Devuelve la causa (M26: descartes.tsv).
    fn discard_original(&mut self, st: &Status) -> &'static str {
        match st {
            Status::Timeout => {
                self.a_timeout += 1;
                "timeout"
            }
            Status::Signal => {
                self.a_signal += 1;
                "senal"
            }
            _ => {
                self.a_exit += 1;
                "exit"
            }
        }
    }

    fn line(&self) -> String {
        format!(
            "total={} iguales={} DIVERGENCIAS={} conocidas={} conocidas_sintoma_nuevo={} PANICS={} RECURSOS={} (timeout={} memoria={}) LINT={} observados={} observados_por_globals={} lanzaron={} | descartados: original={} (exit={} timeout={} senal={} invalido={} observado_sin_salida={} mudo_sin_observar={} enlace={} no_determinista={}) mudos_sin_bindings={} parse={} (esm={} otro={} script={}) codegen={} runner={} (a={} b={}) ilegibles={} panics_del_fuzzer={} | identicos={} lint_equivalentes={} cache_a_hits={}",
            self.total,
            self.same,
            self.divergences,
            self.known,
            self.known_drift,
            self.panics,
            self.recursos_timeout + self.recursos_memoria,
            self.recursos_timeout,
            self.recursos_memoria,
            self.lint,
            self.observed,
            self.observed_globals,
            self.threw,
            self.a_exit
                + self.a_timeout
                + self.a_signal
                + self.a_invalid
                + self.observed_not_ok
                + self.silent_no_observe
                + self.a_link
                + self.a_nondet,
            self.a_exit,
            self.a_timeout,
            self.a_signal,
            self.a_invalid,
            self.observed_not_ok,
            self.silent_no_observe,
            self.a_link,
            self.a_nondet,
            self.silent_unobservable,
            self.parse_esm + self.parse_other + self.parse_script,
            self.parse_esm,
            self.parse_other,
            self.parse_script,
            self.codegen_error,
            self.runner_failed,
            self.runner_a,
            self.runner_failed - self.runner_a,
            self.unreadable,
            self.fuzzer_panics,
            self.identical,
            self.lint_triaged,
            self.cache_hits,
        )
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // M34: subcomando oculto, el hijo que minifica (subproceso.rs).
    if args.first().map(String::as_str) == Some("--preparar") {
        std::process::exit(subproceso::serve(&args[1..]));
    }
    // A6: subcomando oculto, el hijo que analiza con el parser de SWC (analisis.rs).
    if args.first().map(String::as_str) == Some("--analizar") {
        std::process::exit(analisis::serve());
    }
    // R2-H09: la ayuda es un pedido, no un error de uso (antes: "opcion desconocida", 2).
    // Solo como argumento propio: `--out -h` sigue por Cli::parse (pide_ayuda).
    if pide_ayuda(&args) {
        println!("{USAGE}");
        return;
    }
    let repetidas = opciones_repetidas(&args);
    let cli = match Cli::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(SALIDA_USO);
        }
    };
    // Antes ganaba la ultima en silencio; sigue ganando, con aviso.
    for (o, v) in repetidas {
        eprintln!("aviso: {o} repetida, gana la ultima: {o} {v}");
    }
    // `--cruce DIR`: solo lee los indices, no corre nada. Sin index-*.tsv: exit 1.
    if let Some(dir) = &cli.cruce {
        match indice::cruce(dir) {
            Ok(t) => print!("{t}"),
            Err(e) => {
                eprintln!("no pude leer los indices en {}: {e}", dir.display());
                std::process::exit(SALIDA_ERROR);
            }
        }
        return;
    }

    if let Some(f) = &cli.emitir {
        std::panic::set_hook(Box::new(minify::record_panic_location));
        match emitir::emitir(f, cli.minifier, cli.preset, cli.forma, cli.kind) {
            Ok(t) => print!("{t}"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(SALIDA_ERROR);
            }
        }
        return;
    }

    // Un panic del minifier es un hallazgo, no ruido: se captura en minify::prepare
    // y se guarda. El hook default lo imprimiria en stderr por cada caso.
    // M22: el hook guarda donde fue el panic (archivo:linea:col del crate) para
    // que minify::prepare lo agregue al mensaje.
    std::panic::set_hook(Box::new(minify::record_panic_location));

    install_signal_handlers();
    // Subreaper (Linux): los hijos `detached` de los programas no sobreviven a su
    // ejecucion (huerfanos.rs).
    huerfanos::activar();
    if cli.subproceso {
        subproceso::activar();
    }
    // R1-H05: el oraculo sin debug assertions. Sin el, los panics van a panic/
    // como antes, con el aviso una sola vez aca.
    let mut vigilados: Vec<PathBuf> = std::env::current_exe().into_iter().collect();
    match subproceso::configurar_oraculo() {
        Ok(Some(p)) => {
            eprintln!("oraculo sin debug assertions: {}", p.display());
            if let Some(aviso) = subproceso::aviso_viejo(&p) {
                eprintln!("aviso: {aviso}");
            }
            vigilados.push(p);
        }
        Ok(None) => {}
        Err(aviso) => eprintln!("aviso: {aviso}"),
    }
    // R1-H18: si este binario o el oraculo cambian a mitad de corrida, los hijos
    // que se lancen despues serian otro codigo (binario.rs).
    for sin in binario::fijar(&vigilados) {
        eprintln!("aviso: no puedo vigilar si cambia {sin}");
    }
    let stale = runner::clean_stale();
    if stale > 0 {
        eprintln!("borre {stale} temporales de corridas anteriores cortadas (pid muerto)");
    }

    // M27 (d): "no existe", "no es directorio" y "sin input.js" son distintos.
    if let Err(e) = corpus::check_root(&cli.corpus) {
        eprintln!("{e}");
        std::process::exit(SALIDA_ERROR);
    }
    let files = corpus::collect(&cli.corpus);
    if files.is_empty() {
        eprintln!("no encontre ningun input.js bajo {}", cli.corpus.display());
        std::process::exit(SALIDA_ERROR);
    }
    let files: Vec<PathBuf> = match cli.limit {
        Some(n) => files.into_iter().take(n).collect(),
        None => files,
    };
    // n-sandbox: antes de tocar la salida, que el sandbox pedido se pueda armar.
    if let Err(e) = runner::probar_sandbox(cli.sandbox) {
        eprintln!("{e}");
        std::process::exit(SALIDA_ERROR);
    }

    let preload = match runner::Preload::write() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("no pude escribir el preload: {e}");
            std::process::exit(SALIDA_ERROR);
        }
    };
    let cell = cli.cell();
    // R1-H09: un directorio por celda dentro de --out. R2-H05: la marca de
    // corrida queda en incompleta antes de que el reporter borre lo anterior.
    let cell_out = oracle::dir_celda(&cli.out, &cell);
    if let Some(aviso) = oracle::aviso_salida_vieja(&cli.out) {
        eprintln!("aviso: {aviso}");
    }
    if let Some(e) = corrida::estado(&cell_out).filter(|e| e != "completa") {
        eprintln!("aviso: la corrida anterior de {cell} no termino ({e}); la reemplazo");
    }
    let corrida = corrida::Corrida::iniciar(
        &cell_out,
        &[
            ("celda", cell.clone()),
            ("comando", std::env::args().collect::<Vec<_>>().join(" ")),
            ("corpus", cli.corpus.display().to_string()),
            ("casos", files.len().to_string()),
            ("difuzz", env!("CARGO_PKG_VERSION").to_string()),
            ("node", cache::node_version()),
            ("backend", cli.minifier.version().to_string()),
        ],
    );
    let errors_before = oracle::write_errors();
    let mut reporter = oracle::Reporter::new(&cell_out, cell.clone(), runner::PRELOAD)
        .with_kind(cli.kind)
        .with_invocar(cli.invocar)
        .with_versions(&cache::node_version(), cli.minifier.version());
    if let Some(dir) = &cli.indice {
        match indice::Index::create(dir, &cell) {
            Ok(ix) => reporter = reporter.with_index(ix),
            Err(e) => eprintln!("no pude crear el indice en {}: {e}", dir.display()),
        }
    }
    // Todo lo que cambia la ejecucion de A ademas del texto; la celda no: es
    // lo que permite compartir la cache entre celdas. El carril entra porque
    // cambia como node ejecuta el archivo (.cjs o .mjs).
    let timeout_ms = cli.timeout.as_millis().to_string();
    let kind_tag = format!("carril_{} .{}", cli.kind.carril(), cli.kind.ext());
    // M11: "entorno v2" (cwd/env/TMPDIR/HOME aislados): lo que ve el programa cambio.
    // M30: con stubs, A no es la misma ejecucion (lo que no enlazaba ahora corre).
    let mut cache_parts = vec![runner::PRELOAD, timeout_ms.as_str(), kind_tag.as_str(), "entorno v2"];
    if cli.stubs {
        cache_parts.push("stubs v1");
    }
    // 09-oct: sin el sensor de violaciones, A no deja las mismas lineas en el canal
    // (flags `*_atrapada`, `viol`): no puede compartir cache con una celda que lo usa.
    if !cli.minifier.politica().usa_sensor() {
        cache_parts.push("sin sensor");
    }
    // n-sandbox: con el sandbox, A no es la misma ejecucion (lo que niega, falla).
    match cli.sandbox {
        runner::Sandbox::Apagado => {}
        runner::Sandbox::Permisos => cache_parts.push("sandbox v1"),
        runner::Sandbox::PermisosYRed => cache_parts.push("sandbox red v1"),
    }
    let cache = cache::CacheA::new(cli.cache.clone(), &cache_parts);
    // M27 (a): si --out no se puede escribir, no hay hallazgos que guardar.
    if oracle::write_errors() > errors_before {
        eprintln!("no puedo escribir en --out {}", cell_out.display());
        // exit no corre los Drop: el preload (y su directorio) se sueltan a mano.
        drop(preload);
        std::process::exit(SALIDA_ERROR);
    }
    let triage = triage::Triage::load(&cli.out);
    let known = known::Known::load(
        &cli.corpus,
        &cell_out,
        // M4: "sintoma v2" (sin rutas por proceso): las tablas viejas se
        // descartan por el encabezado en vez de dar drift masivo.
        known::fnv1a(&[runner::PRELOAD, cell.as_str(), "sintoma v2"]),
        cli.minifier,
    );
    let ctx = Ctx {
        reporter: &reporter,
        known: &known,
        cache: &cache,
        triage: &triage,
        // 7a: que re-chequeos aplican lo dice la politica del backend (clasificar.rs).
        medido: Some(medido_de_celda(&cli)),
    };
    let jobs = cli.jobs.min(files.len());
    // R2-H04: el presupuesto de memoria de los hijos, con los workers reales.
    if cli.subproceso {
        eprintln!("{}", subproceso::configurar_memoria(jobs, cli.mem_hijo));
    }

    eprintln!(
        "difuzz · {} casos · config {} · {jobs} workers · timeout {:?} · observador {} · backlog conocido {} · salida en {} · cache {}",
        files.len(),
        cell,
        cli.timeout,
        if cli.observe { "on" } else { "off" },
        known.backlog_len(),
        cell_out.display(),
        cache.dir().map_or("off".into(), |d| d.display().to_string())
    );

    // Cola de trabajo: un contador atomico sobre el Vec ordenado. Cada worker
    // tiene su runner (path propio), su config (MangleOptions::default pasa por
    // serde_json; se paga una vez por worker, no por caso) y sus stats.
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let total = files.len();
    let mut stats = Stats::default();

    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..jobs)
            .filter_map(|w| {
                let (files, next, done, ctx, cli, preload) =
                    (&files, &next, &done, &ctx, &cli, &preload);
                // M1: un desborde de pila no se atrapa con catch_unwind (aborta el
                // proceso entero). 256 MiB alcanzan para los 9 casos anidados
                // conocidos; es reserva virtual, no RSS.
                let spawned = std::thread::Builder::new()
                    .name(format!("worker-{w}"))
                    .stack_size(WORKER_STACK)
                    .spawn_scoped(scope, move || {
                    let (cfg, names_cfg) = configs_de_celda(cli);
                    let runner = runner_de_celda(preload, w, cli);
                    let mut stats = Stats::default();
                    loop {
                        // R1-H18: con el binario cambiado, no se toma otro caso.
                        if CANCEL.load(Ordering::SeqCst) || binario::cambio().is_some() {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = files.get(i) else {
                            break;
                        };
                        // Un panic nuestro (no del minifier: ese lo captura
                        // minify::prepare) pierde el caso, no el worker ni sus stats.
                        analisis::tomar_notas();
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            process_case(ctx, &runner, &cfg, names_cfg.as_ref(), cli, &mut stats, path)
                        }));
                        if r.is_err() {
                            stats.fuzzer_panics += 1;
                            eprintln!("panic del fuzzer en {}", path.display());
                            // R2-H06: tambien en descartes.tsv (la identidad del USAGE).
                            let rel = path.strip_prefix(&cli.corpus).unwrap_or(path);
                            ctx.reporter.discard(rel, "panic_del_fuzzer", Duration::ZERO);
                        }
                        // A6/R1-H12: un fallo del analisis no es hallazgo del minifier.
                        let notas = analisis::tomar_notas();
                        if let Some(n) = notas.first() {
                            stats.fallos_analisis += 1;
                            eprintln!("analisis de difuzz fallo en {}: {}", path.display(), n.lines().next().unwrap_or_default());
                        }
                        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if n % 500 == 0 {
                            eprintln!("[{n}/{total}]");
                        }
                    }
                    stats
                });
                match spawned {
                    Ok(h) => Some(h),
                    Err(e) => {
                        eprintln!("no pude crear el worker {w}: {e}");
                        None
                    }
                }
            })
            .collect();
        if workers.is_empty() {
            eprintln!("no hay workers: nada que correr");
            std::process::exit(SALIDA_ERROR);
        }
        for w in workers {
            match w.join() {
                Ok(s) => stats.merge(&s),
                Err(_) => eprintln!("un worker murio por panic; sus casos no se cuentan"),
            }
        }
    });

    let cancelled = CANCEL.load(Ordering::SeqCst);
    // R1-H18: el binario o el oraculo cambiaron a mitad de corrida (binario.rs).
    let cambiado = binario::cambio();
    let late = huerfanos::cosechar();
    if late > 0 {
        eprintln!("subreaper: mate {late} procesos huerfanos al final de la corrida");
    }
    reporter.finish(!cancelled && cambiado.is_none());
    if let Err(e) = known.save() {
        eprintln!("no pude guardar known-sintomas.tsv: {e}");
    }
    if let Err(e) = triage.save() {
        eprintln!("no pude guardar lint-triage.tsv: {e}");
    }
    print_hint(&cell_out, &stats);
    println!("{}", stats.line());
    if cli.mutaciones + cli.mutaciones_extra > 0 {
        println!("{}", stats.mutaciones.line());
    }
    if cli.mutaciones_extra > 0 {
        println!("{}", stats.mutaciones.line_extra());
    }
    if cli.invocar {
        println!("invocar (M31): reintentos_sin_invocar={}", stats.invocar_retry);
    }
    // R1-H13 / R1-H14: en una linea aparte (no cambia la de `total=`).
    println!(
        "analisis_swc (R1-H13): recuperados={} perdidos={} lint_sin_analisis={} | bloques (R1-H14): pasadas={} sin_epilogo_normal={} sin_salida={}",
        stats.analisis_recuperado, stats.analisis_perdido, stats.lint_sin_analisis, stats.bloques, stats.bloques_unica, stats.bloques_sin_salida
    );
    if subproceso::hay_oraculo() || stats.solo_asserts > 0 {
        println!("oraculo (R1-H05): solo_asserts={} sin_divergencia={}", stats.solo_asserts, stats.solo_asserts_sin_div);
    }
    if stats.fallos_analisis > 0 {
        println!("analisis de difuzz (A6/R1-H12): casos_con_fallo={} descartados={}", stats.fallos_analisis, stats.descartes_analisis);
    }
    // M1 (revision-b456): en una linea aparte, solo si paso (no cambia la de `total=`).
    if stats.sensor_repetidos > 0 {
        println!("sensor (I3): comparados_sin_sensor={}", stats.sensor_repetidos);
    }
    // n-sandbox: en una linea aparte, solo con --sandbox (no cambia la de `total=`).
    let linea_sandbox = cli.sandbox.activo().then(|| format!("sandbox: modo={} descartados={}", cli.sandbox.nombre(), stats.a_sandbox));
    if let Some(l) = &linea_sandbox {
        println!("{l}");
    }
    // M27 (a): una escritura perdida del reporter es un hallazgo perdido.
    let lost = oracle::write_errors() - errors_before;
    // R2-H05: solo una corrida entera (todos los casos contados: un worker
    // muerto pierde los suyos) y sin escrituras perdidas es `completa`.
    let mut notas = vec![format!("procesados: {} de {total}", stats.total), format!("stats: {}", stats.line())];
    notas.extend(linea_sandbox);
    if lost > 0 {
        notas.push(format!("escrituras_perdidas: {lost}"));
    }
    if let Some(c) = &cambiado {
        notas.push(format!("binario_cambiado: {c}"));
    }
    let entera = lost == 0 && stats.total == total && cambiado.is_none();
    corrida.cerrar(if cancelled { "interrumpida" } else if entera { "completa" } else { "incompleta" }, &notas);
    if lost > 0 && !cancelled {
        eprintln!("el reporter no pudo escribir {lost} archivos en {}: la corrida no es confiable", cell_out.display());
        drop(preload);
        std::process::exit(SALIDA_ERROR);
    }
    if cancelled {
        eprintln!("corrida interrumpida: {} de {total} casos; el indice anterior quedo intacto", stats.total);
        drop(preload);
        std::process::exit(SALIDA_INTERRUMPIDA);
    }
    // R1-H18 (b6-infra): antes que el "incompleta" generico, para que el mensaje diga por que.
    if let Some(c) = cambiado {
        eprintln!("corrida cortada (R1-H18): {c}; {} de {total} casos, el indice anterior quedo intacto", stats.total);
        drop(preload);
        std::process::exit(SALIDA_ERROR);
    }
    // R2-H09: CORRIDA.txt dice incompleta (un worker murio y sus casos no se
    // contaron): antes salia con 0.
    if !entera {
        eprintln!("corrida incompleta: {} de {total} casos contados en {}", stats.total, cell_out.display());
        drop(preload);
        std::process::exit(SALIDA_ERROR);
    }
}

/// M23: SIGINT/SIGTERM piden cortar: los workers terminan el caso actual (el
/// runner mata el grupo de cada ejecucion, M2) y no toman otro; despues se
/// guardan known-sintomas.tsv y lint-triage.tsv y se descarta el indice a
/// medias. Una segunda senal sale en el acto (los restos los borra la proxima
/// corrida, runner::clean_stale).
static CANCEL: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    if CANCEL.swap(true, Ordering::SeqCst) {
        // SAFETY: _exit es async-signal-safe.
        unsafe { libc::_exit(SALIDA_INTERRUMPIDA) };
    }
}

fn install_signal_handlers() {
    let handler: extern "C" fn(libc::c_int) = on_signal;
    for sig in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: el handler solo toca un AtomicBool y llama a _exit.
        unsafe {
            libc::signal(sig, handler as libc::sighandler_t);
        }
    }
}

/// M1: pila de cada worker. Un fuente muy anidado (parse, minify, codegen o el
/// reparseo de checks) desborda la pila por defecto (2 MiB) y aborta la corrida.
const WORKER_STACK: usize = 256 << 20;

/// Un caso del corpus, de punta a punta. Corre en el worker que lo tomo.
fn process_case(
    ctx: &Ctx,
    runner: &runner::Runner,
    cfg: &minify::Config,
    names_cfg: Option<&minify::Config>,
    cli: &Cli,
    stats: &mut Stats,
    path: &Path,
) {
    stats.total += 1;
    let rel = path.strip_prefix(&cli.corpus).unwrap_or(path);

    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            stats.unreadable += 1;
            // R2-H06: cada caso que no termina en un destino tiene su fila.
            ctx.reporter.discard(rel, "ilegible", Duration::ZERO);
            return;
        }
    };
    let src = cli.forma.apply(&raw);

    // R1-H05: la nota del oraculo es por caso (minify::nota_asserts).
    minify::tomar_nota_asserts();
    stats.nota_pasada = None;
    let (original, minified, static_flags, dead_writes, analisis) = match minify::prepare(&src, cfg) {
        minify::Outcome::Ok {
            original,
            minified,
            static_flags,
            dead_writes,
            analisis,
        } => (original, minified, static_flags, dead_writes, analisis),
        minify::Outcome::ParseError { esm, script } => {
            let cause = if esm {
                stats.parse_esm += 1;
                "parse_esm"
            } else if script {
                stats.parse_script += 1;
                "parse_script"
            } else {
                stats.parse_other += 1;
                "parse_otro"
            };
            ctx.reporter.discard(rel, cause, Duration::ZERO);
            return;
        }
        minify::Outcome::CodegenError => {
            stats.codegen_error += 1;
            ctx.reporter.discard(rel, "codegen", Duration::ZERO);
            return;
        }
        minify::Outcome::Panic(msg) => {
            report_panic(ctx, stats, rel, &src, &msg);
            return;
        }
    };

    // R1-H13: el analisis con el parser de SWC no es silencioso.
    let lint_valido = contar_analisis(stats, analisis);
    // Lint sobre la pasada cruda (sin observador: el epilogo apagaria el
    // disparador). No depende de ejecutar nada.
    if lint_valido && dead_writes.1 > dead_writes.0 {
        match ctx.triage.check(rel, &minified) {
            triage::Lint::Equivalente => stats.lint_triaged += 1,
            triage::Lint::Reportar(note) => {
                stats.lint += 1;
                eprintln!(
                    "LINT escrituras sobre literales {} -> {}: {}",
                    dead_writes.0,
                    dead_writes.1,
                    rel.display()
                );
                let note = con_nota_analisis(note, analisis);
                ctx.reporter.lint(rel, &original, &minified, dead_writes, &note);
            }
        }
    }

    // M30: el manifiesto de stubs sale del original y vale para todo el caso.
    if cli.stubs {
        runner.set_stubs(&analisis::stubs(&original, cli.kind));
    }
    // Precondicion del oraculo: el original termina solo (exit 0 o lanzando).
    let (a, a_cached) = ctx.cache.run(runner, &original, &mut stats.cache_hits);
    if original_no_compila(runner, &a, &original) {
        stats.a_invalid += 1;
        ctx.reporter.discard(rel, "invalido", a.elapsed);
        return;
    }
    if let Status::Failed(e) = &a.status {
        stats.runner_failed += 1;
        stats.runner_a += 1;
        ctx.reporter.discard(rel, "runner", a.elapsed);
        eprintln!("runner: {e}");
        return;
    }
    if descarte_sandbox(ctx, runner, stats, rel, &a) {
        return;
    }

    // Mejora 10: variantes, solo si el original termina solo (si no, las
    // variantes casi seguro tampoco, y cada una costaria un timeout). M18: y no
    // si es mudo sin globals (las variantes se comparan solo por la pasada
    // cruda: serian mudas), y DESPUES de comparar el base, para no reportar una
    // variante que diverge igual que su base (dedup por kind).
    let mutate = cli.mutaciones + cli.mutaciones_extra > 0 && a.completed() && !(a.ok_silent() && !a.has_globals());
    let base_lint = dead_writes.1 > dead_writes.0;
    stats.last_div = None;
    compare_base(ctx, runner, cfg, names_cfg, cli, stats, rel, &raw, &original, &minified, &static_flags, a, a_cached);
    // R1-H05: con debug assertions hubo panic y el oraculo minifico. Si diverge,
    // ya esta en div/ (o known/) con la nota; si no, va a asserts/, no a panic/.
    if let Some(nota) = minify::tomar_nota_asserts() {
        stats.solo_asserts += 1;
        if stats.last_div.is_none() {
            stats.solo_asserts_sin_div += 1;
            ctx.reporter.solo_asserts(rel, &src, &original, &minified, &nota);
        }
    }
    if mutate {
        let base_div = stats.last_div.take();
        process_mutants(ctx, runner, cfg, names_cfg, cli, &mut stats.mutaciones, rel, &raw, base_div, base_lint);
    }
}

/// La comparacion del caso base (pasada cruda y, si es mudo, la observada).
#[allow(clippy::too_many_arguments)]
fn compare_base(
    ctx: &Ctx,
    runner: &runner::Runner,
    cfg: &minify::Config,
    names_cfg: Option<&minify::Config>,
    cli: &Cli,
    stats: &mut Stats,
    rel: &Path,
    raw: &str,
    original: &str,
    minified: &str,
    static_flags: &[&'static str],
    a: runner::Exec,
    a_cached: bool,
) {
    if a.ok_with_output() {
        if a.threw() {
            stats.threw += 1;
        }
        if !compare_and_report(ctx, runner, stats, rel, original, minified, a, a_cached, false, static_flags, names_cfg) {
            stats.same += 1;
        }
    } else if a.ok_silent() {
        // Programa mudo. Primero la comparacion cruda: si el minificado lanza,
        // se cuelga o imprime algo, ya es divergencia (asi se ve un SyntaxError
        // como el de swc#12397 en un programa que no imprime). Los globals que
        // creo (lineas `!dz global`) entran en esta comparacion.
        let had_globals = a.has_globals();
        if compare_and_report(ctx, runner, stats, rel, original, minified, a, a_cached, false, static_flags, names_cfg) {
            return;
        }
        if !cli.observe {
            stats.silent_no_observe += 1;
            ctx.reporter.discard(rel, "mudo_sin_observar", Duration::ZERO);
            return;
        }
        // M12: en el carril B, primero el envoltorio que serializa los exports;
        // un modulo sin exports cae al mismo epilogo que el carril A.
        if cli.kind == minify::SourceKind::Module
            && observe_module(ctx, runner, stats, rel, original, minified, static_flags, names_cfg, cli.invocar)
        {
            return;
        }
        observe_epilogue(ctx, runner, cfg, names_cfg, cli, stats, rel, raw, had_globals, cli.invocar);
    } else if a.link_failed() {
        // M9: en los dos carriles (en A, un require de algo que no existe).
        stats.a_link += 1;
        ctx.reporter.discard(rel, "enlace", a.elapsed);
    } else {
        let cause = stats.discard_original(&a.status);
        ctx.reporter.discard(rel, cause, a.elapsed);
    }
}

/// Segunda pasada de un programa mudo: el epilogo observador (minify::instrument)
/// sobre el fuente crudo, minificado y comparado de nuevo. Carril A, y desde M12
/// tambien el carril B cuando el modulo no tiene exports.
#[allow(clippy::too_many_arguments)]
fn observe_epilogue(
    ctx: &Ctx,
    runner: &runner::Runner,
    cfg: &minify::Config,
    names_cfg: Option<&minify::Config>,
    cli: &Cli,
    stats: &mut Stats,
    rel: &Path,
    raw: &str,
    had_globals: bool,
    invocar: bool,
) {
    // Segunda pasada con el epilogo observador.
    // El epilogo se agrega al fuente crudo y despues se aplica la forma: asi
    // queda dentro de la funcion y ve los bindings.
    // R2-H01: esta pasada es SIEMPRE la de sin `--invocar` (epilogo_normal):
    // cualquier referencia que agregue el epilogo cambia lo que el minifier
    // puede optimizar (la re-observacion de M31 c le impedia a oxc inlinear y
    // se perdia 6422/2). `--invocar` es una pasada adicional (invoke_pass).
    let instrumented = match epilogo_normal(raw, cli) {
        Ok(i) => i,
        // A6: el analisis (no el minifier) fallo: descarte con causa explicita.
        Err(e) => {
            analisis::registrar("epilogo observador", &e);
            stats.descartes_analisis += 1;
            ctx.reporter.discard(rel, "analisis", Duration::ZERO);
            return;
        }
    };
    let Some(instrumented) = instrumented else {
        // R1-H14: sin bindings directos, quizas los hay en bloques.
        let antes = stats.bloques;
        match pasada_bloques(ctx, runner, cfg, names_cfg, cli, stats, rel, raw) {
            Ok(Some(cerrado)) => {
                // Un panic al minificar cierra el caso sin compararlo.
                if stats.bloques > antes {
                    stats.observed += 1;
                    stats.bloques_unica += 1;
                }
                if !cerrado {
                    stats.same += 1;
                }
                return;
            }
            Ok(None) => {}
            // b2-int (A6): fallo el analisis de la pasada de bloques. Sin
            // epilogo normal ni globals observados no se sabe si el mudo tenia
            // bindings en bloques: descarte con causa `analisis`, como cuando
            // falla el epilogo normal. Con globals, el caso ya se observo.
            Err(e) => {
                analisis::registrar("epilogo de la pasada de bloques", &e);
                if !had_globals {
                    stats.descartes_analisis += 1;
                    ctx.reporter.discard(rel, "analisis", Duration::ZERO);
                    return;
                }
            }
        }
        if had_globals {
            stats.observed_globals += 1;
            stats.same += 1;
        } else {
            stats.silent_unobservable += 1;
            // R2-H06: mudo sin nada que observar, con su fila en descartes.tsv.
            ctx.reporter.discard(rel, "mudo_sin_bindings", Duration::ZERO);
        }
        return;
    };
    let (original, minified, static_flags) = match minify::prepare(&instrumented, cfg) {
        minify::Outcome::Ok {
            original,
            minified,
            static_flags,
            analisis,
            ..
        } => {
            stats.nota_pasada = analisis.nota();
            (original, minified, static_flags)
        }
        minify::Outcome::Panic(msg) => {
            report_panic(ctx, stats, rel, &instrumented, &msg);
            return;
        }
        // El fuente ya parseo sin el epilogo; si falla con el, es problema
        // del instrumentador, no un hallazgo.
        minify::Outcome::ParseError { .. } | minify::Outcome::CodegenError => {
            stats.silent_unobservable += 1;
            ctx.reporter.discard(rel, "mudo_sin_bindings", Duration::ZERO);
            return;
        }
    };
    let (a, a_cached) = ctx.cache.run(runner, &original, &mut stats.cache_hits);
    if descarte_sandbox(ctx, runner, stats, rel, &a) {
        return;
    }
    if !a.ok_with_output() {
        stats.observed_not_ok += 1;
        ctx.reporter.discard(rel, "observado_sin_salida", a.elapsed);
        return;
    }
    stats.observed += 1;
    if a.threw() {
        stats.threw += 1;
    }
    if compare_and_report(ctx, runner, stats, rel, &original, &minified, a, a_cached, true, &static_flags, names_cfg) {
        return;
    }
    if invocar && invoke_pass(ctx, runner, cfg, names_cfg, cli, stats, rel, raw, &instrumented) {
        return;
    }
    // R1-H14: los bindings en bloques, en una pasada adicional.
    match pasada_bloques(ctx, runner, cfg, names_cfg, cli, stats, rel, raw) {
        Ok(Some(true)) => return,
        Ok(_) => {}
        // b2-int (A6): la pasada normal ya comparo; el caso sigue con la nota.
        Err(e) => analisis::registrar("epilogo de la pasada de bloques (vale la pasada normal)", &e),
    }
    stats.same += 1;
}

/// R1-H08 / R2-H04: un `Outcome::Panic` que en realidad es un hijo que no
/// termino a tiempo o que paso el presupuesto de memoria va a `recursos/` con
/// su causa; el resto, a `panic/` como siempre.
fn report_panic(ctx: &Ctx, stats: &mut Stats, rel: &Path, src: &str, msg: &str) {
    match subproceso::causa_recurso(msg) {
        Some(causa) => {
            if causa == "timeout" {
                stats.recursos_timeout += 1;
            } else {
                stats.recursos_memoria += 1;
            }
            ctx.reporter.recurso(rel, src, causa, msg);
        }
        None => {
            stats.panics += 1;
            ctx.reporter.panic(rel, src, msg);
        }
    }
}

/// R1-H14: pasada ADICIONAL de un programa mudo que observa solo los bindings
/// top-level que el epilogo normal no ve (`var` en bloques, funciones de
/// Anexo B: minify::instrument_bloques). No reemplaza al epilogo normal, que
/// queda byte a byte igual: sumarle nombres agrega referencias y le cambia al
/// minifier lo que puede inlinear o borrar (R2-H01). Corre si la normal dio
/// iguales o si no habia epilogo normal. `None`: no hay nada que observar o el
/// original con este epilogo no termino con salida (no cuenta). `Some(true)`:
/// cerro el caso (divergencia, conocida, panic, falla del runner).
/// `Some(false)`: comparado, iguales.
///
/// b2-int (A6): el epilogo de bloques lo arma el hijo de analisis (operacion
/// `bloques`: parsea con SWC). `Err`: ese analisis fallo (el llamador anota y
/// decide; nunca es un hallazgo del minifier).
#[allow(clippy::too_many_arguments)]
fn pasada_bloques(
    ctx: &Ctx,
    runner: &runner::Runner,
    cfg: &minify::Config,
    names_cfg: Option<&minify::Config>,
    cli: &Cli,
    stats: &mut Stats,
    rel: &Path,
    raw: &str,
) -> Result<Option<bool>, String> {
    let Some(epilogo) = analisis::bloques(raw, cli.kind)? else {
        return Ok(None);
    };
    let instrumented = cli.forma.apply(&epilogo);
    let (original, minified, static_flags) = match minify::prepare(&instrumented, cfg) {
        minify::Outcome::Ok { original, minified, static_flags, analisis, .. } => {
            let mut notas: Vec<String> = analisis.nota().into_iter().collect();
            notas.push("pasada de bloques (R1-H14): el epilogo observa solo los bindings en bloques (var, funciones de Anexo B)".into());
            stats.nota_pasada = Some(notas.join(" | "));
            (original, minified, static_flags)
        }
        minify::Outcome::Panic(msg) => {
            // b2-int: timeout o memoria del minificador van a recursos/ (R1-H08).
            report_panic(ctx, stats, rel, &instrumented, &msg);
            return Ok(Some(true));
        }
        minify::Outcome::ParseError { .. } | minify::Outcome::CodegenError => return Ok(None),
    };
    let (a, a_cached) = ctx.cache.run(runner, &original, &mut stats.cache_hits);
    if descarte_sandbox(ctx, runner, stats, rel, &a) {
        return Ok(Some(true));
    }
    if !a.ok_with_output() {
        stats.bloques_sin_salida += 1;
        return Ok(None);
    }
    stats.bloques += 1;
    if a.threw() {
        stats.threw += 1;
    }
    Ok(Some(compare_and_report(ctx, runner, stats, rel, &original, &minified, a, a_cached, true, &static_flags, names_cfg)))
}

/// R1-H13: cuenta el estado del analisis de la pasada cruda y deja su nota
/// para meta.txt. Devuelve si el lint puede correr (estan los dos lados).
fn contar_analisis(stats: &mut Stats, analisis: checks::Analisis) -> bool {
    stats.nota_pasada = analisis.nota();
    if analisis.perdido() {
        stats.analisis_perdido += 1;
    } else if !analisis.limpio() {
        stats.analisis_recuperado += 1;
    }
    let valido = analisis.lint_valido();
    if !valido {
        stats.lint_sin_analisis += 1;
    }
    valido
}

/// R1-H13: la nota del triage del lint mas la del analisis, si la hay.
fn con_nota_analisis(note: String, analisis: checks::Analisis) -> String {
    match analisis.nota() {
        Some(n) => format!("{note}\n{n}"),
        None => note,
    }
}

/// R2-H01: el epilogo de la pasada observada normal. No depende de `--invocar`:
/// es byte a byte el de una corrida sin `--invocar`.
fn epilogo_normal(raw: &str, cli: &Cli) -> Result<Option<String>, String> {
    analisis::epilogo(raw, false, cli.kind).map(|o| o.map(|s| cli.forma.apply(&s)))
}

/// R2-H01: la pasada de `--invocar`, ADICIONAL a la normal y solo si esa dio
/// iguales: el epilogo que invoca las funciones y re-observa (M31) sobre el
/// mismo fuente crudo, minificado y comparado aparte. Devuelve `true` si cerro
/// el caso (divergencia, conocida, panic, falla del runner). M31 (a): si el
/// original con este epilogo no termina bien (una funcion invocada se cuelga o
/// sale con otro codigo), vale la pasada normal (`invocar_retry`).
#[allow(clippy::too_many_arguments)]
fn invoke_pass(
    ctx: &Ctx,
    runner: &runner::Runner,
    cfg: &minify::Config,
    names_cfg: Option<&minify::Config>,
    cli: &Cli,
    stats: &mut Stats,
    rel: &Path,
    raw: &str,
    normal: &str,
) -> bool {
    let instrumented = analisis::epilogo(raw, true, cli.kind).unwrap_or_else(|e| {
        analisis::registrar("epilogo de --invocar (vale la pasada normal)", &e);
        None
    });
    let Some(instrumented) = instrumented.map(|s| cli.forma.apply(&s)) else {
        return false;
    };
    // M31 (b): sin por donde invocar, el epilogo es el normal: ya se comparo.
    if instrumented == normal {
        return false;
    }
    let (original, minified, static_flags) = match minify::prepare(&instrumented, cfg) {
        minify::Outcome::Ok {
            original,
            minified,
            static_flags,
            analisis,
            ..
        } => {
            stats.nota_pasada = analisis.nota();
            (original, minified, static_flags)
        }
        minify::Outcome::Panic(msg) => {
            report_panic(ctx, stats, rel, &instrumented, &msg);
            return true;
        }
        minify::Outcome::ParseError { .. } | minify::Outcome::CodegenError => return false,
    };
    let (a, a_cached) = ctx.cache.run(runner, &original, &mut stats.cache_hits);
    if descarte_sandbox(ctx, runner, stats, rel, &a) {
        return true;
    }
    if !a.ok_with_output() {
        stats.invocar_retry += 1;
        return false;
    }
    compare_and_report(ctx, runner, stats, rel, &original, &minified, a, a_cached, true, &static_flags, names_cfg)
}

/// Carril B, programa mudo: segunda pasada por el envoltorio que serializa el
/// namespace de exports (runner::module_wrapper). El fuente y el minificado son
/// los mismos de la pasada cruda: no hay epilogo, no hay efecto observador.
#[allow(clippy::too_many_arguments)]
fn observe_module(
    ctx: &Ctx,
    runner: &runner::Runner,
    stats: &mut Stats,
    rel: &Path,
    original: &str,
    minified: &str,
    static_flags: &[&'static str],
    names_cfg: Option<&minify::Config>,
    invocar: bool,
) -> bool {
    let mut obs = runner.observing();
    // M28: cacheada con el tag del envoltorio (cache.rs).
    let (mut a, mut a_cached) = ctx.cache.run(&obs, original, &mut stats.cache_hits);
    // M31 (a): si invocar los exports no termina, la observacion sin invocar.
    if invocar && !a.completed() {
        stats.invocar_retry += 1;
        obs = runner.observing().with_invocar(false);
        (a, a_cached) = ctx.cache.run(&obs, original, &mut stats.cache_hits);
    }
    if descarte_sandbox(ctx, runner, stats, rel, &a) {
        return true;
    }
    // Sin exports el envoltorio no imprime nada. M12: el llamador sigue con el
    // epilogo (antes el caso quedaba en mudos_sin_bindings: 585 en B contra 84 en A).
    if a.ok_silent() {
        return false;
    }
    if !a.ok_with_output() {
        stats.observed_not_ok += 1;
        ctx.reporter.discard(rel, "observado_sin_salida", a.elapsed);
        return true;
    }
    stats.observed += 1;
    if a.threw() {
        stats.threw += 1;
    }
    if !compare_and_report(ctx, &obs, stats, rel, original, minified, a, a_cached, true, static_flags, names_cfg) {
        stats.same += 1;
    }
    true
}

/// El runner de un worker de la celda. Decision del usuario (09-oct): el sensor
/// de violaciones del preload lo decide la politica del backend medido
/// (`Politica::usa_sensor`): swc corre sin el (A, B y re-chequeos), oxc con el.
fn runner_de_celda(preload: &runner::Preload, worker: usize, cli: &Cli) -> runner::Runner {
    runner::Runner::new(preload, worker, cli.timeout)
        .with_kind(cli.kind)
        .with_invocar(cli.invocar)
        .with_stubs(cli.stubs)
        .with_sensor(cli.minifier.politica().usa_sensor())
        .with_sandbox(cli.sandbox)
}

/// n-sandbox: si esta ejecucion del original toco el sandbox (un permiso negado, atrapado
/// o no), el caso no se compara: va a descartes con causa `sandbox`, nunca a div/ ni a
/// known/ por eso. Vale para cualquier pasada (cruda, observada, de bloques, `--invocar`,
/// envoltorio de modulos). Sin `--sandbox`, `Runner::toco_sandbox` es siempre false.
fn descarte_sandbox(ctx: &Ctx, runner: &runner::Runner, stats: &mut Stats, rel: &Path, a: &runner::Exec) -> bool {
    if !runner.toco_sandbox(a) {
        return false;
    }
    stats.a_sandbox += 1;
    ctx.reporter.discard(rel, "sandbox", a.elapsed);
    true
}

/// La config de minificacion de un worker y la de nombres preservados. Solo
/// core_defaults cambia nombres: ahi cada divergencia se vuelve a probar con los
/// nombres preservados (names_only). (M5: la usa tambien el e2e.)
fn configs_de_celda(cli: &Cli) -> (minify::Config, Option<minify::Config>) {
    let cfg = cli.preset.build(cli.minifier, cli.kind);
    let names_cfg = (cli.preset == minify::Preset::CoreDefaults).then(|| minify::Config::con_nombres(cli.minifier, cli.preset, cli.kind));
    (cfg, names_cfg)
}

/// El backend medido de la celda, para los re-chequeos de su politica (`Ctx::medido`).
fn medido_de_celda(cli: &Cli) -> clasificar::Medido {
    clasificar::Medido { minifier: cli.minifier, preset: cli.preset, kind: cli.kind }
}

/// Lo que comparten todas las comparaciones de una corrida.
struct Ctx<'a> {
    reporter: &'a oracle::Reporter,
    known: &'a known::Known,
    cache: &'a cache::CacheA,
    triage: &'a triage::Triage,
    /// 7a: el backend medido y su celda, para los re-chequeos que declara su
    /// politica (M13 flags estaticos, M17 solo sloppy) y re-minificar con el.
    /// `None` (tests): ningun re-chequeo propio del backend.
    medido: Option<clasificar::Medido>,
}

/// Corre B y compara contra A (que ya termino con exit 0). Devuelve `true` si
/// el caso quedo cerrado: divergencia reportada o falla del runner. `false`
/// significa "iguales"; el llamador decide si eso cuenta como `same` o si sigue
/// a la pasada observada.
#[allow(clippy::too_many_arguments)]
fn compare_and_report(
    ctx: &Ctx,
    runner: &runner::Runner,
    stats: &mut Stats,
    rel: &Path,
    original: &str,
    minified: &str,
    a: runner::Exec,
    a_cached: bool,
    observed: bool,
    static_flags: &[&'static str],
    names_cfg: Option<&minify::Config>,
) -> bool {
    // Mismo texto, mismo path, mismo preload: B daria lo mismo que A.
    if minified == original {
        if !observed {
            stats.identical += 1;
        }
        return false;
    }
    let b = runner.run(minified);
    // I3 (revision-b456): el sensor cambia lo que el programa observa (V8 formatea el
    // stack al pausar: llama al toString del programa y congela `.stack`). Es simetrico
    // solo si A y B corrieron igual. Si alguno corrio sin el sensor (`sensor_caido`: se
    // repitio sin el tras una senal o un timeout) o a medias (`sensor_tope`: MAX_PAUSAS),
    // los dos se repiten sin el sensor (salvo el que ya corrio entero sin el) y todo lo
    // que sigue (confirmacion y re-chequeos) usa el runner sin sensor.
    let (mut a, mut b, mut a_cached) = (a, b, a_cached);
    let sin_sensor;
    let mut runner = runner;
    let mut paso_sensor = None;
    if runner.con_sensor() {
        let (da, db) = (a.sensor_degradado(), b.sensor_degradado());
        if da.is_some() || db.is_some() {
            sin_sensor = runner.sin_sensor();
            runner = &sin_sensor;
            stats.sensor_repetidos += 1;
            if da != Some(runner::SENSOR_CAIDO) {
                a = runner.run(original);
                a_cached = false;
            }
            if db != Some(runner::SENSOR_CAIDO) {
                b = runner.run(minified);
            }
            let lado = |d: Option<&str>| match d {
                Some(runner::SENSOR_CAIDO) => "sin el sensor (sensor_caido: senal o timeout con el)",
                Some(_) => "con el sensor a medias (sensor_tope: MAX_PAUSAS)",
                None => "con el sensor",
            };
            paso_sensor = Some(format!(
                "A {}, B {} -> A y B comparados sin el sensor (I3)",
                lado(da),
                lado(db)
            ));
        }
    }
    if let Status::Failed(e) = &b.status {
        stats.runner_failed += 1;
        ctx.reporter.discard(rel, "runner", b.elapsed);
        eprintln!("runner: {e}");
        return true;
    }
    let (a, b) = match confirm(ctx, runner, stats, rel, original, minified, a, a_cached, b) {
        Confirmed::Same => return false,
        Confirmed::Nondeterministic => return true,
        Confirmed::Divergence(ab) => *ab,
    };
    match oracle::compare(&a, &b) {
        oracle::Verdict::Same => false,
        oracle::Verdict::Divergence(kind) => {
            stats.last_div = Some(kind);
            if stats.skip_kind == Some(kind) {
                stats.inherited += 1;
                return true;
            }
            // 7d: la decision div/ o known/ (y por que), con su traza (clasificar.rs).
            let c = clasificar::clasificar(
                ctx,
                runner,
                rel,
                original,
                minified,
                &a,
                &b,
                kind,
                static_flags,
                names_cfg,
                stats.nota_pasada.as_deref(),
            );
            if c.drift {
                stats.known_drift += 1;
            }
            let mut flags_b: Vec<&str> = b.flags.iter().map(String::as_str).collect();
            clasificar::sin_repetidos(&mut flags_b);
            let clasificar::Clasificacion { flags, why, nota, mut traza, .. } = c;
            // M1 (revision-b456): que el sensor no corrio entero queda en meta.txt.
            if let Some(p) = paso_sensor {
                traza.anteponer("sensor", p);
            }
            if why.is_some() {
                stats.known += 1;
            } else {
                stats.divergences += 1;
                let tag = if observed { " (observado)" } else { "" };
                eprintln!("DIVERGENCIA {kind:?}{tag}: {}", rel.display());
            }
            ctx.reporter.divergence(
                rel,
                original,
                minified,
                &a,
                &b,
                kind,
                observed,
                why.as_deref(),
                nota.as_deref(),
                &flags,
                &flags_b,
                if a_cached { "hit (confirmado con una corrida fresca)" } else { "miss" },
                &traza.texto(),
            );
            true
        }
    }
}

/// M3: resultado de confirmar una comparacion antes de clasificarla.
enum Confirmed {
    Same,
    /// A no se reproduce a si mismo: el caso se descarta (`no_determinista`).
    Nondeterministic,
    /// La divergencia sigue con este A y este B.
    Divergence(Box<(runner::Exec, runner::Exec)>),
}

/// A y A' dicen lo mismo para el oraculo (status y stdout normalizado).
fn same_exec(x: &runner::Exec, y: &runner::Exec) -> bool {
    x.status == y.status && oracle::normalize_names(&x.stdout) == oracle::normalize_names(&y.stdout)
}

/// M3: ninguna divergencia se reporta sin confirmar.
/// - B en Timeout: se reintenta con `max(4 x timeout, 20 x a.elapsed)`; si
///   ahora termina, se compara con eso (un Timeout por carga no es hallazgo).
/// - A de la cache: se corre A en fresco. Si difiere, la entrada esta envenenada
///   (depende de ppid, worker, cwd, otra corrida): lapida y se sigue con el A fresco.
/// - Si la divergencia sigue y A no fue confirmado por una corrida fresca, se
///   corre A otra vez: si no se reproduce, el caso es no determinista.
#[allow(clippy::too_many_arguments)]
fn confirm(
    ctx: &Ctx,
    runner: &runner::Runner,
    stats: &mut Stats,
    rel: &Path,
    original: &str,
    minified: &str,
    a: runner::Exec,
    a_cached: bool,
    b: runner::Exec,
) -> Confirmed {
    let diverges = |a: &runner::Exec, b: &runner::Exec| matches!(oracle::compare(a, b), oracle::Verdict::Divergence(_));
    let (mut a, mut b) = (a, b);
    if !diverges(&a, &b) {
        return Confirmed::Same;
    }
    if b.status == Status::Timeout {
        let longer = (runner.timeout() * 4).max(a.elapsed * 20);
        let b2 = runner.run_with_timeout(minified, longer);
        if !matches!(b2.status, Status::Timeout | Status::Failed(_)) {
            b = b2;
            if !diverges(&a, &b) {
                return Confirmed::Same;
            }
        }
    }
    let mut fresh_ok = false;
    if a_cached {
        let a1 = runner.run(original);
        if same_exec(&a1, &a) {
            // M21: el A fresco (su elapsed es de esta corrida, no de la cacheada).
            a = a1;
            fresh_ok = true;
        } else if !matches!(a1.status, Status::Failed(_)) {
            ctx.cache.tombstone(runner, original);
            a = a1;
            if !diverges(&a, &b) {
                return Confirmed::Same;
            }
        }
    }
    if !fresh_ok {
        let a2 = runner.run(original);
        if !matches!(a2.status, Status::Failed(_)) && !same_exec(&a2, &a) {
            stats.a_nondet += 1;
            ctx.reporter.discard(rel, "no_determinista", a2.elapsed);
            return Confirmed::Nondeterministic;
        }
    }
    Confirmed::Divergence(Box::new((a, b)))
}

/// Contadores de las variantes (mejora 10). R1-H15: `comparadas` = iguales +
/// divergencias + conocidas + heredadas + fallas del runner en B (`runner - runner_a`);
/// `no_determinista` es un descarte y queda fuera (ver `contar_variante`).
#[derive(Default)]
struct MutStats {
    variantes: usize,
    comparadas: usize,
    iguales: usize,
    divergencias: usize,
    conocidas: usize,
    lint: usize,
    panics: usize,
    /// R1-H08 / R2-H04: timeout o memoria del hijo (`recursos/`).
    recursos: usize,
    /// El original de la variante no imprime nada: sin observador, no hay que comparar.
    mudas: usize,
    /// El original de la variante no termina solo (timeout, exit, senal) o node no lo acepta.
    original: usize,
    /// R1-H15: la variante diverge pero su original corrido otra vez en fresco da
    /// otra salida (mismo criterio que el caso, `confirm` / M3): no es comparable.
    /// Antes contaba como `iguales`. Tambien va a descartes.tsv (`no_determinista`).
    no_determinista: usize,
    /// La variante no pasa por el minifier (parse o codegen).
    parse: usize,
    runner: usize,
    /// M32: las de A de la variante. Las de B son `runner - runner_a`.
    runner_a: usize,
    /// M15: casos cuyo base no parsea en el carril de la corrida: 0 variantes.
    no_parsea: usize,
    /// M18: variantes que divergen con el mismo kind que su base: no se reportan.
    heredadas: usize,
    /// M33: variantes de los operadores nuevos (incluidas en `variantes`).
    extra: usize,
    /// M33: variantes `var` <-> `let`/`const` cuyo original prende `tdz`: ruido del
    /// operador, se descartan sin comparar.
    tdz: usize,
}

impl MutStats {
    fn merge(&mut self, o: &MutStats) {
        self.variantes += o.variantes;
        self.comparadas += o.comparadas;
        self.iguales += o.iguales;
        self.divergencias += o.divergencias;
        self.conocidas += o.conocidas;
        self.lint += o.lint;
        self.panics += o.panics;
        self.recursos += o.recursos;
        self.mudas += o.mudas;
        self.original += o.original;
        self.no_determinista += o.no_determinista;
        self.parse += o.parse;
        self.runner += o.runner;
        self.runner_a += o.runner_a;
        self.no_parsea += o.no_parsea;
        self.heredadas += o.heredadas;
        self.extra += o.extra;
        self.tdz += o.tdz;
    }

    /// M33: aparte, para no cambiar la linea de siempre.
    fn line_extra(&self) -> String {
        format!("mutaciones extra (M33): variantes={} | descartadas: tdz={}", self.extra, self.tdz)
    }

    fn line(&self) -> String {
        format!(
            "mutaciones: variantes={} comparadas={} iguales={} DIVERGENCIAS={} conocidas={} heredadas={} PANICS={} RECURSOS={} LINT={} | descartadas: mudas={} original={} no_determinista={} parse={} runner={} (a={} b={}) | bases_sin_parse={}",
            self.variantes,
            self.comparadas,
            self.iguales,
            self.divergencias,
            self.conocidas,
            self.heredadas,
            self.panics,
            self.recursos,
            self.lint,
            self.mudas,
            self.original,
            self.no_determinista,
            self.parse,
            self.runner,
            self.runner_a,
            self.runner - self.runner_a,
            self.no_parsea,
        )
    }
}

/// Mejora 10: variantes del caso (mutate.rs) por la pasada cruda + lint, nunca
/// por el observador (el epilogo agrega usos y apaga el lint; una variante muda
/// se descarta). Cada variante es su propio caso, `<caso>__mutK`, y su fuente
/// queda tal cual en `variante.js` junto al hallazgo.
#[allow(clippy::too_many_arguments)]
fn process_mutants(
    ctx: &Ctx,
    runner: &runner::Runner,
    cfg: &minify::Config,
    names_cfg: Option<&minify::Config>,
    cli: &Cli,
    m: &mut MutStats,
    rel: &Path,
    raw: &str,
    base_div: Option<oracle::Kind>,
    base_lint: bool,
) {
    // M15: segun el carril. Un base que no parsea queda contado (no_parsea).
    // M33: las extra van despues de las clasicas y se numeran desde
    // `--mutaciones` + 1 (el pedido, no lo generado): una clasica `__mutK` es
    // la misma con o sin extras.
    let vs = match analisis::variantes(raw, cli.mutaciones, cli.mutaciones_extra, known::fnv1a(&[raw]), cli.kind) {
        Ok(vs) => vs,
        // A6: el analisis (no el minifier) fallo: sin variantes, con nota.
        Err(e) => {
            analisis::registrar("mutaciones (sin variantes en este caso)", &e);
            return;
        }
    };
    let Some(vs) = vs else {
        m.no_parsea += 1;
        return;
    };
    let mut extras = 0;
    for (k, v) in vs.into_iter().enumerate() {
        m.variantes += 1;
        let idx = if v.extra {
            m.extra += 1;
            extras += 1;
            cli.mutaciones + extras
        } else {
            k + 1
        };
        let rel = mutate::rel_variante(rel, idx);
        let desc = format!("{}\n", v.desc);
        let files = [("variante.js", v.src.as_str()), ("mutacion.txt", desc.as_str())];
        let src = cli.forma.apply(&v.src);
        minify::tomar_nota_asserts();
        let (original, minified, static_flags, dead_writes, analisis) = match minify::prepare(&src, cfg) {
            minify::Outcome::Ok {
                original,
                minified,
                static_flags,
                dead_writes,
                analisis,
            } => (original, minified, static_flags, dead_writes, analisis),
            minify::Outcome::Panic(msg) => {
                // R1-H08 / R2-H04: un timeout o la memoria del hijo, a recursos/.
                if let Some(causa) = subproceso::causa_recurso(&msg) {
                    m.recursos += 1;
                    ctx.reporter.recurso(&rel, &src, causa, &msg);
                } else {
                    m.panics += 1;
                    ctx.reporter.panic(&rel, &src, &msg);
                }
                ctx.reporter.attach(&rel, &files);
                continue;
            }
            minify::Outcome::ParseError { .. } | minify::Outcome::CodegenError => {
                m.parse += 1;
                continue;
            }
        };
        // M18: si el base ya fue lint, el de la variante casi siempre es el mismo.
        // R1-H13: sin los dos lados analizados, el lint no dice nada.
        if analisis.lint_valido() && dead_writes.1 > dead_writes.0 && !base_lint {
            // Mismo triage que el caso (lint-triage.tsv): una variante
            // equivalente ya mirada no vuelve a lint/.
            if let triage::Lint::Reportar(note) = ctx.triage.check(&rel, &minified) {
                m.lint += 1;
                eprintln!(
                    "LINT escrituras sobre literales {} -> {}: {}",
                    dead_writes.0,
                    dead_writes.1,
                    rel.display()
                );
                let note = con_nota_analisis(note, analisis);
                ctx.reporter.lint(&rel, &original, &minified, dead_writes, &note);
                ctx.reporter.attach(&rel, &files);
            }
        }
        if cli.stubs {
            runner.set_stubs(&analisis::stubs(&original, cli.kind));
        }
        let a = runner.run(&original);
        if let Status::Failed(e) = &a.status {
            m.runner += 1;
            m.runner_a += 1;
            eprintln!("runner: {e}");
            continue;
        }
        // n-sandbox: el original de la variante toco el sandbox: no se compara (cuenta en
        // `original=` de las mutaciones, como las demas variantes cuyo original no sirve).
        if runner.toco_sandbox(&a) {
            m.original += 1;
            continue;
        }
        if v.tdz_filter && a.flags.iter().any(|f| f == "tdz") {
            m.tdz += 1;
            continue;
        }
        if a.ok_silent() {
            m.mudas += 1;
            continue;
        }
        // Mismo criterio que process_case: tambien el RangeError de compilacion.
        if !a.ok_with_output() || original_no_compila(runner, &a, &original) {
            m.original += 1;
            continue;
        }
        let mut s = Stats { skip_kind: base_div, nota_pasada: analisis.nota(), ..Stats::default() };
        compare_and_report(ctx, runner, &mut s, &rel, &original, &minified, a, false, false, &static_flags, names_cfg);
        if contar_variante(m, &s) {
            ctx.reporter.attach(&rel, &files);
        }
    }
}

/// El original no es un programa que node compile: lanzo un SyntaxError antes de
/// imprimir nada y `node --check` lo rechaza (node no lo acepta como script), o
/// R2-H03: un RangeError que `node --check` reproduce (V8 no pudo compilar el
/// fuente: anidamiento extremo, 20000 `[`). Un RangeError en runtime (recursion,
/// `new Array(-1)`) pasa `node --check` y se compara. Lo usan el caso
/// (`invalido`) y las variantes (`original=` de las mutaciones).
fn original_no_compila(runner: &runner::Runner, a: &runner::Exec, original: &str) -> bool {
    a.threw()
        && (a.stdout.starts_with("!dz throw SyntaxError") || a.stdout.starts_with("!dz throw RangeError"))
        && !runner.node_accepts(original)
}

/// R1-H15: suma a `m` lo que dejo `compare_and_report` para una variante.
/// Devuelve `true` si hay hallazgo (div/ o known/) al que adjuntar la variante.
/// Una variante cuyo A no se reproduce (`confirm` la descarto: `a_nondet`) no
/// cuenta como comparada ni como igual: es un descarte con causa.
fn contar_variante(m: &mut MutStats, s: &Stats) -> bool {
    if s.a_nondet > 0 {
        m.no_determinista += 1;
        return false;
    }
    m.comparadas += 1;
    m.divergencias += s.divergences;
    m.conocidas += s.known;
    m.heredadas += s.inherited;
    m.runner += s.runner_failed;
    if s.inherited > 0 {
        return false;
    }
    if s.divergences + s.known > 0 {
        return true;
    }
    if s.runner_failed == 0 {
        m.iguales += 1;
    }
    false
}

fn print_hint(out: &Path, stats: &Stats) {
    for l in hint_lines(out, stats) {
        eprintln!("{l}");
    }
}

/// Una linea por directorio que la corrida lleno. Antes, con solo panics,
/// decia "hallazgos en div" y nunca nombraba `panic/`; y no contaba lo que
/// dejan las variantes de --mutaciones (van a los mismos directorios).
fn hint_lines(out: &Path, stats: &Stats) -> Vec<String> {
    let m = &stats.mutaciones;
    let o = out.display();
    let mut v = Vec::new();
    if stats.divergences + m.divergencias > 0 {
        v.push(format!("hallazgos (divergencias) en {o}/div"));
    }
    if stats.panics + m.panics > 0 {
        v.push(format!("hallazgos (panics del minifier) en {o}/panic"));
    }
    if stats.known + m.conocidas > 0 {
        v.push(format!("conocidas (asuncion o backlog) en {o}/known"));
    }
    if stats.lint + m.lint > 0 {
        v.push(format!("sospechosos del lint (triage a mano) en {o}/lint"));
    }
    if stats.recursos_timeout + stats.recursos_memoria + m.recursos > 0 {
        v.push(format!("minifier lento o sin memoria (timeout/memoria) en {o}/recursos"));
    }
    if stats.solo_asserts_sin_div > 0 {
        v.push(format!("panics solo de debug assertions, sin divergencia, en {o}/asserts"));
    }
    v
}

#[cfg(test)]
mod tests {
    /// El aviso final nombra los directorios que de verdad tienen hallazgos:
    /// con solo panics, `panic/` y no `div/` (antes decia "hallazgos en div").
    #[test]
    fn aviso_nombra_los_directorios_con_hallazgos() {
        let out = std::path::Path::new("/o");
        let solo_panic = super::Stats { panics: 1, ..super::Stats::default() };
        assert_eq!(super::hint_lines(out, &solo_panic), ["hallazgos (panics del minifier) en /o/panic"]);
        let solo_div = super::Stats { divergences: 2, ..super::Stats::default() };
        assert_eq!(super::hint_lines(out, &solo_div), ["hallazgos (divergencias) en /o/div"]);
        let mut mut_panic = super::Stats::default();
        mut_panic.mutaciones.panics = 1;
        mut_panic.mutaciones.divergencias = 1;
        let l = super::hint_lines(out, &mut_panic);
        assert_eq!(l, ["hallazgos (divergencias) en /o/div", "hallazgos (panics del minifier) en /o/panic"]);
        assert!(super::hint_lines(out, &super::Stats::default()).is_empty());
    }

    /// Decision del usuario (09-oct): el runner de la celda (el de produccion,
    /// `runner_de_celda`) corre el sensor de violaciones solo si la politica del
    /// backend lo usa. swc: node sin el sensor (DIFUZZ_SENSOR=0), sin flags ni
    /// lineas `viol` de una TDZ atrapada; oxc: con. Lo mismo vale para los
    /// re-chequeos y la pasada observada, que salen de este runner.
    #[test]
    fn sensor_segun_la_politica() {
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = crate::runner::Preload::write_tagged("sensor-pol").unwrap();
        let prog = "try { x; } catch (e) {}\nlet x = 1;\nconsole.log(1);\n";
        for (m, con) in [("swc", false), ("oxc", true)] {
            let cli = super::Cli::parse(["c", "--minifier", m, "--no-cache"].iter().map(|s| s.to_string()).collect()).unwrap();
            assert_eq!(cli.minifier.politica().usa_sensor(), con, "{m}");
            let r = super::runner_de_celda(&preload, crate::test_util::id_runner(), &cli);
            assert_eq!(r.con_sensor(), con, "{m}");
            assert_eq!(r.observing().con_sensor(), con, "{m}");
            let e = r.run(prog);
            assert_eq!(e.stdout, "1\n", "{m}");
            assert_eq!(e.flags.iter().any(|f| f == "tdz_atrapada"), con, "{m}: {:?}", e.flags);
            assert_eq!(e.ctl.lines().any(|l| l.starts_with("viol tdz ")), con, "{m}: {}", e.ctl);
            // sin_sensor (I3) apaga el sensor aunque la politica lo use.
            let e = r.sin_sensor().run(prog);
            assert!(!e.flags.iter().any(|f| f == "tdz_atrapada"), "{m}: {:?}", e.flags);
        }
    }

    /// I3 / M1 (revision-b456): el sensor cambia lo que el programa observa (al pausar,
    /// V8 formatea el stack y llama al toString del programa). A pasa MAX_PAUSAS antes
    /// del throw y B (sin el loop, que no hace nada visible) no: con el sensor, B
    /// imprime "toString llamado" y A no. Antes eso era una divergencia falsa; ahora los
    /// dos se repiten sin el sensor y dan lo mismo. Con un bug de verdad, div/ y el
    /// meta dice que se compararon sin el sensor.
    #[test]
    fn sensor_asimetrico_se_repite_sin_el() {
        use super::{compare_and_report, Ctx, Stats};
        use crate::{cache, known, oracle, runner, triage};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-i3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, super::backend::Minifier::Oxc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let preload = runner::Preload::write_tagged("i3").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let e = "class E extends Error { toString() { console.log(\"toString llamado\"); return \"E\"; } }\ntry { throw new E(\"x\"); } catch (e) {}\n";
        let original = format!("for (let i = 0; i < 300; i++) {{ try {{ null.x; }} catch (e) {{}} }}\n{e}console.log(\"fin\");\n");
        let minified = format!("{e}console.log(\"fin\");\n");
        let a = r.run(&original);
        assert_eq!((a.stdout.as_str(), a.sensor_degradado()), ("fin\n", Some(runner::SENSOR_TOPE)), "{}", a.ctl);
        let b = r.run(&minified);
        assert_eq!((b.stdout.as_str(), b.sensor_degradado()), ("toString llamado\nfin\n", None), "sin esto el test no prueba nada");
        let mut st = Stats::default();
        assert!(!compare_and_report(&ctx, &r, &mut st, Path::new("igual/input.js"), &original, &minified, a, false, false, &[], None));
        assert_eq!((st.sensor_repetidos, st.divergences, st.a_nondet), (1, 0, 0));
        // Con un bug de verdad: div/, y el paso `sensor` en la traza de meta.txt.
        let bug = format!("{e}console.log(\"fin2\");\n");
        let a = r.run(&original);
        assert!(compare_and_report(&ctx, &r, &mut st, Path::new("bug/input.js"), &original, &bug, a, false, false, &[], None));
        assert_eq!((st.sensor_repetidos, st.divergences), (2, 1));
        let meta = std::fs::read_to_string(d.join("div/bug/meta.txt")).unwrap();
        assert!(
            meta.contains("\nclasificacion:\n  sensor: A con el sensor a medias (sensor_tope: MAX_PAUSAS), B con el sensor -> A y B comparados sin el sensor (I3)\n"),
            "{meta}"
        );
        // Sin sensor por politica (swc) no hay nada que repetir.
        let mut st = Stats::default();
        let sin = r.sin_sensor();
        let a = sin.run(&original);
        assert!(!compare_and_report(&ctx, &sin, &mut st, Path::new("swc/input.js"), &original, &minified, a, false, false, &[], None));
        assert_eq!(st.sensor_repetidos, 0);
        drop((sin, r));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn la_celda_incluye_el_backend() {
        let cli = |args: &[&str]| super::Cli::parse(args.iter().map(|s| s.to_string()).collect()).unwrap();
        // B5 (09-oct): sin --minifier, oxc.
        assert_eq!(cli(&["corpus"]).cell(), "oxc+carril_a+tal_cual");
        assert_eq!(cli(&["corpus", "--minifier", "swc"]).cell(), "swc+carril_a+tal_cual");
        assert_eq!(
            cli(&["corpus", "--minifier", "oxc", "--config", "core_defaults", "--forma", "en_funcion"]).cell(),
            "oxc+core_defaults+en_funcion"
        );
        assert!(super::Cli::parse(vec!["corpus".into(), "--minifier".into(), "terser".into()]).is_err());
    }

    #[test]
    fn minificado_identico_no_corre_b() {
        use super::{cache, compare_and_report, known, oracle, runner, triage, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-identico-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, super::backend::Minifier::Swc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx {
            reporter: &reporter,
            known: &known,
            cache: &cache,
            triage: &triage,
            medido: None,
        };
        let preload = runner::Preload::write_tagged("identico").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(5));
        // A inventado: si B corriera de verdad, imprimiria "1" y divergeria.
        let a = || runner::Exec {
            status: runner::Status::Exited(0),
            stdout: "no es lo que imprime\n".into(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: std::time::Duration::ZERO,
            ctl: String::new(),
        };
        let js = "console.log(1);\n";
        let mut st = Stats::default();
        let rel = Path::new("x/input.js");
        assert!(!compare_and_report(&ctx, &r, &mut st, rel, js, js, a(), false, false, &[], None));
        assert_eq!((st.identical, st.divergences), (1, 0));
        // En la pasada observada tambien se saltea, pero no se cuenta dos veces.
        assert!(!compare_and_report(&ctx, &r, &mut st, rel, js, js, a(), false, true, &[], None));
        assert_eq!(st.identical, 1);
        // Control: con un minificado distinto B si corre y diverge contra el A
        // inventado. M3: antes de reportar se corre A en fresco, que no da lo
        // inventado: el caso se cierra (true) como no determinista, no como div.
        assert!(compare_and_report(&ctx, &r, &mut st, rel, js, "console.log(2);\n", a(), false, false, &[], None));
        assert_eq!((st.identical, st.divergences, st.a_nondet), (1, 0, 1));
        // Con el A real la divergencia se confirma.
        let real = r.run(js);
        assert!(compare_and_report(&ctx, &r, &mut st, rel, js, "console.log(2);\n", real, false, false, &[], None));
        assert_eq!((st.divergences, st.a_nondet), (1, 1));
        // M18 (a): una variante que diverge con el mismo kind que su base no se reporta.
        let mut sk = Stats { skip_kind: Some(oracle::Kind::Stdout), ..Stats::default() };
        let real = r.run(js);
        assert!(compare_and_report(&ctx, &r, &mut sk, rel, js, "console.log(2);\n", real, false, false, &[], None));
        assert_eq!((sk.inherited, sk.divergences, sk.last_div), (1, 0, Some(oracle::Kind::Stdout)));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// b1-b (R1-H03): un minificado que node no acepta nunca va a known/, tenga
    /// los flags que tenga. Con el mismo flag y un minificado valido, sigue en known/.
    #[test]
    fn minificado_invalido_nunca_es_asuncion() {
        use super::{backend::Minifier, cache, compare_and_report, known, oracle, runner, triage, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-invalido-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, Minifier::Oxc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let preload = runner::Preload::write_tagged("invalido").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        // `coercion` es un flag estatico de todo el programa (05-coercion-miente).
        let o = "class P { toString() { return \"p\"; } }\nconsole.log(1);\n";
        let mut st = Stats::default();
        assert!(compare_and_report(&ctx, &r, &mut st, Path::new("valido/input.js"), o, "console.log(2);\n", r.run(o), false, false, &["coercion"], None));
        assert_eq!((st.known, st.divergences), (1, 0));
        let mut st = Stats::default();
        assert!(compare_and_report(&ctx, &r, &mut st, Path::new("invalido/input.js"), o, "console.log(1;\n", r.run(o), false, false, &["coercion"], None));
        assert_eq!((st.known, st.divergences), (0, 1));
        let meta = std::fs::read_to_string(d.join("div/invalido/meta.txt")).unwrap();
        assert!(meta.contains("known: no\n") && meta.contains("nota: node --check rechaza el minificado"), "{meta}");
        assert!(meta.contains("minificado_invalido"), "{meta}");
        // 7d: la traza de la clasificacion, al final de meta.txt.
        assert!(meta.contains("\nclasificacion:\n  profundidad: no aplica"), "{meta}");
        assert!(meta.contains("\n  minificado_invalido: si (node --check rechaza el minificado y acepta el original): sin flags, ninguna\n  decision: div/\n"), "{meta}");
        let meta = std::fs::read_to_string(d.join("known/valido/meta.txt")).unwrap();
        assert!(meta.contains("\n  asunciones: sensor, asuncion: oxc: Coercion Methods Are Pure\n  minificado_invalido: no (node --check acepta el minificado)\n"), "{meta}");
        assert!(meta.ends_with("  decision: known/ (sensor, asuncion: oxc: Coercion Methods Are Pure)\n"), "{meta}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// M31 (a): si invocar no termina (cuelgue, exit propio), la observacion se
    /// repite sin invocar en vez de perder el caso.
    #[test]
    fn invocar_que_no_termina_reintenta_sin_invocar() {
        use super::{backend::Minifier, cache, known, minify, observe_epilogue, oracle, runner, triage, Cli, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-m31a-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, super::backend::Minifier::Swc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let cli = Cli::parse(["c", "--invocar", "--no-cache"].iter().map(|s| s.to_string()).collect()).unwrap();
        let cfg = minify::Preset::CarrilA.build(Minifier::Swc, minify::SourceKind::Script);
        let preload = runner::Preload::write_tagged("m31a").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_millis(1500));
        let rel = Path::new("x/input.js");
        for raw in ["var k = 1;\nfunction f() { while (true) {} }\n", "var k = 1;\nfunction f() { process.exit(3); }\n"] {
            let mut st = Stats::default();
            observe_epilogue(&ctx, &r, &cfg, None, &cli, &mut st, rel, raw, false, true);
            assert_eq!((st.invocar_retry, st.observed, st.observed_not_ok, st.same), (1, 1, 0, 1), "{raw}");
        }
        // Una que termina no se repite.
        let mut st = Stats::default();
        observe_epilogue(&ctx, &r, &cfg, None, &cli, &mut st, rel, "var k = 1;\nfunction f() { return k; }\n", false, true);
        assert_eq!((st.invocar_retry, st.observed), (0, 1));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// R2-H01: con `--invocar`, el epilogo de la pasada normal es byte a byte el
    /// de sin `--invocar` (el de 9f100ab sin la opcion), en los dos carriles y
    /// las dos formas; el de invocacion es otro texto y va en otra pasada.
    #[test]
    fn invocar_no_cambia_el_epilogo_normal() {
        use super::{epilogo_normal, minify, Cli};
        let p = |v: &[&str]| Cli::parse(v.iter().map(|s| s.to_string()).collect()).unwrap();
        let fuentes = [
            "import assert from 'assert'; let result='FAIL'; const unused={...{get prop(){result='PASS'}}}; assert.strictEqual(result,'PASS');\n",
            "var k = 1;\nfunction f() { k = 2; }\n",
            "let o = { n: 0 };\nconst inc = () => o.n++;\nclass C {}\n",
            "module.exports = { f() { return 1; } };\n",
        ];
        let mut comparadas = 0;
        for carril in ["a", "b"] {
            for forma in ["tal_cual", "en_funcion"] {
                let sin = p(&["c", "--carril", carril, "--forma", forma]);
                let con = p(&["c", "--carril", carril, "--forma", forma, "--invocar"]);
                assert!(con.invocar && !sin.invocar);
                for raw in fuentes {
                    let normal = epilogo_normal(raw, &con).unwrap();
                    assert_eq!(normal, epilogo_normal(raw, &sin).unwrap(), "{carril} {forma} {raw}");
                    let referencia = minify::instrument(raw, false, sin.kind).map(|s| sin.forma.apply(&s));
                    assert_eq!(normal, referencia, "{carril} {forma} {raw}");
                    if let Some(n) = normal {
                        assert!(!n.contains("__difuzzObserveInvocar") && !n.contains("__difuzzObserveDespues"), "{n}");
                        let inv = minify::instrument(raw, true, con.kind).map(|s| con.forma.apply(&s)).unwrap();
                        assert_ne!(n, inv);
                        comparadas += 1;
                    }
                }
            }
        }
        assert!(comparadas >= 12, "{comparadas}");
    }

    /// R1-H14: un miscompile que solo toca un `var` declarado en un bloque. El
    /// epilogo normal no lo ve (y queda igual: no observa `b`); la pasada de
    /// bloques si, y va a div/ con la nota. Sin bindings directos, la pasada de
    /// bloques es la unica observacion (antes: mudos_sin_bindings).
    #[test]
    fn pasada_de_bloques_ve_var_en_bloque() {
        use super::{cache, epilogo_normal, known, minify, observe_epilogue, oracle, runner, triage, Cli, Ctx, Stats};
        use crate::backend::Backend;
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        /// "Minifica" cambiando `= 2` por `= 3`: un bug que solo se ve en `b`.
        struct Rompe;
        impl Backend for Rompe {
            fn prepare(&self, src: &str) -> minify::Outcome {
                minify::Outcome::Ok { original: src.into(), minified: src.replace("= 2", "= 3"), static_flags: Vec::new(), dead_writes: (0, 0), analisis: Default::default() }
            }
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-h14-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, super::backend::Minifier::Swc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let cli = Cli::parse(["c", "--no-cache"].iter().map(|s| s.to_string()).collect()).unwrap();
        let cfg = minify::Config::from_backend(Box::new(Rompe));
        let preload = runner::Preload::write_tagged("h14").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let raw = "var a = 1;\nif (a) { var b = 2; }\n";
        let normal = epilogo_normal(raw, &cli).unwrap().unwrap();
        assert!(normal.contains("[\"a\", a]") && !normal.contains("\"b\""), "{normal}");
        let mut st = Stats::default();
        observe_epilogue(&ctx, &r, &cfg, None, &cli, &mut st, Path::new("x/input.js"), raw, false, false);
        assert_eq!((st.observed, st.bloques, st.bloques_unica, st.divergences, st.same), (1, 1, 0, 1, 0));
        let meta = std::fs::read_to_string(d.join("div/x/meta.txt")).unwrap();
        assert!(meta.contains("pasada de bloques (R1-H14)"), "{meta}");
        // Solo en bloques: antes mudos_sin_bindings, ahora observado.
        let mut st = Stats::default();
        observe_epilogue(&ctx, &r, &cfg, None, &cli, &mut st, Path::new("y/input.js"), "{ var b = 2; }\n", false, false);
        assert_eq!((st.observed, st.bloques, st.bloques_unica, st.divergences, st.silent_unobservable), (1, 1, 1, 1, 0));
        // Sin nada en bloques: como antes.
        let mut st = Stats::default();
        observe_epilogue(&ctx, &r, &cfg, None, &cli, &mut st, Path::new("z/input.js"), "var a = 1;\nif (a) { let c = 2; }\n", false, false);
        assert_eq!((st.observed, st.bloques, st.same, st.divergences), (1, 0, 1, 0));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// R2-H03: un original que V8 no puede compilar (RangeError al parsear
    /// 6000 `[` anidados; MEDIDO con node 24: 3000 compila, 3500 no; `node --check` lo rechaza igual) es descarte
    /// `invalido` en los dos backends y carriles, no un ThrowLost en div/. Un
    /// RangeError en runtime pasa `node --check` y se sigue comparando.
    #[test]
    fn original_que_node_no_compila_es_invalido() {
        use super::{backend::Minifier, cache, known, minify, oracle, process_case, runner, triage, Cli, Ctx, Stats, WORKER_STACK};
        if !crate::test_util::requiere_node() {
            return;
        }
        // La pila de un worker: parsear 20000 niveles en el hilo del test no entra.
        std::thread::Builder::new()
            .stack_size(WORKER_STACK)
            .spawn(|| {
                let d = std::env::temp_dir().join(format!("difuzz-test-h03-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&d);
                let corpus = d.join("corpus");
                for (caso, js) in [
                    ("anidado", format!("var a = {}{};console.log(1)\n", "[".repeat(6000), "]".repeat(6000))),
                    ("runtime", "Number(process.argv.length).toFixed(200);\n".to_string()),
                ] {
                    std::fs::create_dir_all(corpus.join(caso)).unwrap();
                    std::fs::write(corpus.join(caso).join("input.js"), js).unwrap();
                }
                let preload = runner::Preload::write_tagged("h03").unwrap();
                let cache = cache::CacheA::new(None, &[]);
                let triage = triage::Triage::load(&d);
                let known = known::Known::load(&d, &d, 0, super::backend::Minifier::Swc);
                for (m, minifier) in [("swc", Minifier::Swc), ("oxc", Minifier::Oxc)] {
                    for (k, kind) in [("a", minify::SourceKind::Script), ("b", minify::SourceKind::Module)] {
                        let out = d.join(format!("out-{m}-{k}"));
                        std::fs::create_dir_all(&out).unwrap();
                        let reporter = oracle::Reporter::new(&out, "test".into(), runner::PRELOAD).with_kind(kind);
                        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
                        let cli = Cli::parse(
                            [corpus.to_str().unwrap(), "--minifier", m, "--carril", k, "--no-cache"].iter().map(|s| s.to_string()).collect(),
                        )
                        .unwrap();
                        let cfg = minify::Preset::CarrilA.build(minifier, kind);
                        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(10)).with_kind(kind);
                        let mut st = Stats::default();
                        process_case(&ctx, &r, &cfg, None, &cli, &mut st, &corpus.join("anidado/input.js"));
                        assert_eq!((st.a_invalid, st.divergences, st.same), (1, 0, 0), "{m}/{k}: {}", st.line());
                        let mut st = Stats::default();
                        process_case(&ctx, &r, &cfg, None, &cli, &mut st, &corpus.join("runtime/input.js"));
                        assert_eq!((st.a_invalid, st.threw, st.same), (0, 1, 1), "{m}/{k}: {}", st.line());
                    }
                }
                std::fs::remove_dir_all(&d).unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// R1-H12/A6, de punta a punta: con un panic forzado en todo el analisis de
    /// difuzz, ningun caso va a panic/ (no es del minifier), con swc y con oxc.
    /// El que imprime se compara sin flags (y sin variantes, con nota); el mudo,
    /// cuyo epilogo depende del analisis, se descarta con causa `analisis`.
    #[test]
    fn analisis_que_falla_no_es_panic_del_minifier() {
        use super::{analisis, backend::Minifier, cache, known, minify, oracle, process_case, runner, triage, Cli, Ctx, Stats};
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-a6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let corpus = d.join("corpus");
        for (caso, js) in [("imprime", "var o = { valueOf() { return 1 } };\nconsole.log(+o, [1][0] = 2);\n"), ("mudo", "var a = 1 + 2;\n")] {
            std::fs::create_dir_all(corpus.join(caso)).unwrap();
            std::fs::write(corpus.join(caso).join("input.js"), js).unwrap();
        }
        let preload = runner::Preload::write_tagged("a6").unwrap();
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let known = known::Known::load(&d, &d, 0, super::backend::Minifier::Swc);
        for (m, minifier) in [("swc", Minifier::Swc), ("oxc", Minifier::Oxc)] {
            let out = d.join(format!("out-{m}"));
            std::fs::create_dir_all(&out).unwrap();
            let reporter = oracle::Reporter::new(&out, "test".into(), runner::PRELOAD);
            let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
            let cli = Cli::parse([corpus.to_str().unwrap(), "--minifier", m, "--no-cache", "--mutaciones", "1"].iter().map(|s| s.to_string()).collect()).unwrap();
            let cfg = minify::Preset::CarrilA.build(minifier, minify::SourceKind::Script);
            let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(10));
            analisis::tomar_notas();
            analisis::forzar_panic(true);
            let mut st = Stats::default();
            process_case(&ctx, &r, &cfg, None, &cli, &mut st, &corpus.join("imprime/input.js"));
            let notas = analisis::tomar_notas();
            process_case(&ctx, &r, &cfg, None, &cli, &mut st, &corpus.join("mudo/input.js"));
            let notas_mudo = analisis::tomar_notas();
            analisis::forzar_panic(false);
            assert_eq!((st.panics, st.same, st.descartes_analisis, st.mutaciones.variantes), (0, 1, 1, 0), "{m}: {}", st.line());
            assert!(!out.join("panic").exists(), "{m}: nada en panic/");
            assert!(notas.iter().any(|n| n.starts_with("chequeos estaticos")) && notas.iter().any(|n| n.starts_with("mutaciones")), "{m}: {notas:?}");
            assert!(notas_mudo.iter().any(|n| n.starts_with("epilogo observador") && n.contains("panic forzado")), "{m}: {notas_mudo:?}");
            // Control: sin forzar, el mudo se observa y el que imprime tiene variantes.
            let mut st = Stats::default();
            process_case(&ctx, &r, &cfg, None, &cli, &mut st, &corpus.join("mudo/input.js"));
            process_case(&ctx, &r, &cfg, None, &cli, &mut st, &corpus.join("imprime/input.js"));
            assert_eq!((st.panics, st.same, st.observed, st.descartes_analisis), (0, 2, 1, 0), "{m}: {}", st.line());
            assert!(st.mutaciones.variantes > 0 && analisis::tomar_notas().is_empty(), "{m}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// R2-H01, de punta a punta: oxc/B con `--invocar` detecta el patron de
    /// 6422/2 (spread con getter que el inlining corre despues del assert), que
    /// la re-observacion de M31 (c) tapaba cuando reemplazaba a la normal.
    #[test]
    fn invocar_no_pierde_el_bug_de_inlining() {
        use super::{backend::Minifier, cache, known, minify, observe_epilogue, oracle, runner, triage, Cli, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-h01-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD).with_kind(minify::SourceKind::Module);
        let known = known::Known::load(&d, &d, 0, super::backend::Minifier::Swc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let cfg = minify::Preset::CoreDefaults.build(Minifier::Oxc, minify::SourceKind::Module);
        let preload = runner::Preload::write_tagged("h01").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(10)).with_kind(minify::SourceKind::Module);
        let rel = Path::new("x/input.js");
        let raw = "import assert from 'assert'; let result='FAIL'; const unused={...{get prop(){result='PASS'}}}; assert.strictEqual(result,'PASS');\n";
        for args in [&["c", "--minifier", "oxc", "--carril", "b", "--no-cache"][..], &["c", "--minifier", "oxc", "--carril", "b", "--no-cache", "--invocar"]] {
            let cli = Cli::parse(args.iter().map(|s| s.to_string()).collect()).unwrap();
            let mut st = Stats::default();
            observe_epilogue(&ctx, &r, &cfg, None, &cli, &mut st, rel, raw, false, cli.invocar);
            assert_eq!((st.observed, st.divergences, st.same, st.invocar_retry), (1, 1, 0, 0), "{args:?}");
        }
        // Un caso sin bug: con --invocar se compara dos veces y cuenta una sola.
        let cli = Cli::parse(["c", "--minifier", "oxc", "--carril", "b", "--no-cache", "--invocar"].iter().map(|s| s.to_string()).collect()).unwrap();
        let mut st = Stats::default();
        observe_epilogue(&ctx, &r, &cfg, None, &cli, &mut st, rel, "let k = 1;\nexport {};\nfunction f() { k = 2; }\nf;\n", false, true);
        assert_eq!((st.observed, st.divergences, st.same), (1, 0, 1));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// R2-H09: `--help`/`-h` cuenta como ayuda en cualquier posicion, menos como el
    /// valor de una opcion. `--out -h` sigue por Cli::parse: sin corpus, error de
    /// uso (2); con corpus, el directorio `-h`.
    #[test]
    fn ayuda_solo_como_argumento_propio() {
        use super::{pide_ayuda, Cli};
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        for a in [&["-h"][..], &["--help"], &["c", "-h"], &["c", "--out", "x", "-h"], &["c", "--jobs", "0", "--help"], &["--out", "-h", "-h"]] {
            assert!(pide_ayuda(&v(a)), "{a:?}");
        }
        for a in [&["--out", "-h"][..], &["c", "--out", "--help"], &["--emitir", "-h"], &["c", "--minifier", "-h"], &["c", "--out"], &[]] {
            assert!(!pide_ayuda(&v(a)), "{a:?}");
        }
        assert_eq!(Cli::parse(v(&["--out", "-h"])).err().as_deref(), Some("falta <corpus_dir>"));
        assert_eq!(Cli::parse(v(&["c", "--out", "-h"])).unwrap().out, std::path::PathBuf::from("-h"));
        assert!(Cli::parse(v(&["c", "--minifier", "-h"])).err().is_some_and(|e| e.starts_with("--minifier invalido: -h")));
    }

    /// Opciones con valor repetidas: Cli::parse se queda con la ultima (como antes)
    /// y opciones_repetidas las nombra, con el valor que gana, para el aviso de main.
    #[test]
    fn opciones_repetidas_avisan_y_gana_la_ultima() {
        use super::{opciones_repetidas, Cli};
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        let a = v(&["c", "--jobs", "2", "--minifier", "oxc", "--jobs", "4", "--minifier", "swc", "--jobs", "3"]);
        let esperado = vec![("--jobs".to_string(), "3".to_string()), ("--minifier".to_string(), "swc".to_string())];
        assert_eq!(opciones_repetidas(&a), esperado);
        let cli = Cli::parse(a).unwrap();
        assert_eq!(cli.jobs, 3);
        assert_eq!(cli.minifier.name(), "swc");
        // Sin aviso: una sola vez cada una, flags sin valor repetidos, y el valor de
        // una opcion que parece opcion (`--out --jobs` es el directorio --jobs).
        for a in [&["c", "--jobs", "2", "--out", "x"][..], &["c", "--no-cache", "--no-cache"], &["c", "--jobs", "2", "--out", "--jobs"], &[]] {
            assert!(opciones_repetidas(&v(a)).is_empty(), "{a:?}");
        }
        assert_eq!(Cli::parse(v(&["c", "--jobs", "2", "--out", "--jobs"])).unwrap().out, std::path::PathBuf::from("--jobs"));
    }

    /// OPCIONES_CON_VALOR al dia con Cli::parse: de cada opcion que nombra el USAGE
    /// y que Cli::parse conoce, toma un valor si y solo si esta en la lista.
    #[test]
    fn opciones_con_valor_al_dia() {
        use super::{Cli, OPCIONES_CON_VALOR, USAGE};
        let mut opciones: Vec<&str> = USAGE
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .filter(|t| t.starts_with("--") && t.len() > 2)
            .collect();
        opciones.sort_unstable();
        opciones.dedup();
        let mut con_valor = 0;
        for o in opciones {
            if o == "--help" {
                continue;
            }
            let e = Cli::parse(vec![o.to_string()]).err().unwrap_or_default();
            if e.starts_with("opcion desconocida") {
                continue;
            }
            let pide = e.starts_with(&format!("{o} necesita"));
            assert_eq!(pide, OPCIONES_CON_VALOR.contains(&o), "{o}: {e}");
            con_valor += usize::from(pide);
        }
        assert_eq!(con_valor, OPCIONES_CON_VALOR.len());
    }

    /// H07 (revision 2): `extends` de A (runtime) y de los estaticos va una sola vez
    /// en `flags_a:` de meta.txt.
    #[test]
    fn flags_a_sin_repetidos_en_meta() {
        use super::{backend::Minifier, cache, compare_and_report, known, oracle, runner, triage, Ctx, Stats};
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-h07-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, Minifier::Oxc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let preload = runner::Preload::write_tagged("h07").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let o = "const v = 1;\nclass A extends v {}\n";
        let a = r.run(o);
        assert_eq!(a.flags, ["extends"], "sin esto el test no prueba nada");
        let mut st = Stats::default();
        assert!(compare_and_report(&ctx, &r, &mut st, Path::new("x/input.js"), o, "console.log(2);\n", a, false, false, &["extends"], None));
        assert_eq!(st.known + st.divergences, 1);
        let dest = if st.known == 1 { "known" } else { "div" };
        let meta = std::fs::read_to_string(d.join(dest).join("x/meta.txt")).unwrap();
        assert!(meta.contains("\nflags_a: extends\n"), "{meta}");
        drop(r);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Mutantes: el original de una variante que V8 no compila (RangeError al
    /// parsear, `node --check` lo rechaza) es `original=`, como `invalido` en
    /// process_case; antes solo el SyntaxError y el RangeError se comparaba. Un
    /// RangeError en runtime se sigue comparando.
    #[test]
    fn variante_que_node_no_compila_no_se_compara() {
        use super::{cache, known, minify, oracle, process_mutants, runner, triage, Cli, Ctx, MutStats};
        use crate::backend::Backend;
        use std::path::Path;
        if !crate::test_util::requiere_node() {
            return;
        }
        /// "Minifica" cualquier variante a un original fijo y un minificado que imprime otra cosa.
        struct Fijo(String);
        impl Backend for Fijo {
            fn prepare(&self, _src: &str) -> minify::Outcome {
                minify::Outcome::Ok { original: self.0.clone(), minified: "console.log(2);\n".into(), static_flags: Vec::new(), dead_writes: (0, 0), analisis: Default::default() }
            }
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-mut-rango-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0, super::backend::Minifier::Swc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let cli = Cli::parse(["c", "--no-cache", "--mutaciones", "2"].iter().map(|s| s.to_string()).collect()).unwrap();
        let preload = runner::Preload::write_tagged("mut-rango").unwrap();
        let r = runner::Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(10));
        let raw = "var a = 1;\nvar b = 2;\nconsole.log(a + b);\n";
        // R2-H03: 6000 `[` anidados no compilan en node 24 (ver original_que_node_no_compila_es_invalido).
        let anidado = format!("var a = {}{};console.log(1)\n", "[".repeat(6000), "]".repeat(6000));
        assert!(!r.node_accepts(&anidado));
        let cfg = minify::Config::from_backend(Box::new(Fijo(anidado)));
        let mut m = MutStats::default();
        process_mutants(&ctx, &r, &cfg, None, &cli, &mut m, Path::new("anidado/input.js"), raw, None, false);
        assert!(m.variantes > 0);
        assert_eq!((m.original, m.comparadas, m.divergencias, m.conocidas), (m.variantes, 0, 0, 0), "{}", m.line());
        assert!(!d.join("div").exists() && !d.join("known").exists());
        // Control: el RangeError en runtime pasa `node --check` y se compara (diverge).
        let cfg = minify::Config::from_backend(Box::new(Fijo("Number(process.argv.length).toFixed(200);\n".into())));
        let mut m = MutStats::default();
        process_mutants(&ctx, &r, &cfg, None, &cli, &mut m, Path::new("runtime/input.js"), raw, None, false);
        assert!(m.variantes > 0);
        assert_eq!((m.original, m.comparadas), (0, m.variantes), "{}", m.line());
        assert_eq!(m.divergencias + m.conocidas, m.variantes, "{}", m.line());
        drop(r);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// M24: --cruce pasa por Cli::parse.
    #[test]
    fn cruce_en_la_cli() {
        use super::Cli;
        let p = |v: &[&str]| Cli::parse(v.iter().map(|s| s.to_string()).collect());
        assert_eq!(p(&["--cruce", "ix"]).unwrap().cruce.as_deref(), Some(std::path::Path::new("ix")));
        assert!(p(&["--cruce", "ix", "--jobs", "2"]).unwrap().cruce.is_some());
        assert!(p(&["--cruce", "ix", "corpus"]).is_err());
        assert!(p(&["--cruce"]).is_err());
        assert!(p(&["corpus"]).unwrap().cruce.is_none());
    }

    /// M27 (b) y (c).
    #[test]
    fn cli_rechaza_timeout_cero_y_dos_corpus() {
        use super::Cli;
        let p = |v: &[&str]| Cli::parse(v.iter().map(|s| s.to_string()).collect());
        assert!(p(&["c", "--timeout-ms", "0"]).err().is_some_and(|e| e.contains(">= 1")));
        assert!(p(&["c", "--timeout-ms", "1"]).is_ok());
        assert!(p(&["c1", "c2"]).err().is_some_and(|e| e.contains("sobra c2")));
    }

    /// M32: `runner=` sigue siendo el total y se desglosa en A y B.
    #[test]
    fn contador_runner_separado() {
        let st = super::Stats { runner_failed: 3, runner_a: 1, ..super::Stats::default() };
        assert!(st.line().contains(" runner=3 (a=1 b=2) "), "{}", st.line());
        let mut t = super::Stats::default();
        t.merge(&st);
        t.merge(&st);
        assert!(t.line().contains(" runner=6 (a=2 b=4) "));
        let m = super::MutStats { runner: 2, runner_a: 2, ..super::MutStats::default() };
        assert!(m.line().contains(" runner=2 (a=2 b=0) "), "{}", m.line());
    }

    /// R1-H15: una variante descartada por no determinista (`confirm`) no cuenta
    /// como igual ni como comparada; las demas salidas de `compare_and_report`
    /// cierran la identidad de `comparadas`.
    #[test]
    fn variante_no_determinista_no_es_igual() {
        use super::{contar_variante, MutStats, Stats};
        let mut m = MutStats::default();
        // Lo que deja `compare_and_report` en cada desenlace de una variante.
        let nondet = Stats { a_nondet: 1, ..Stats::default() };
        assert!(!contar_variante(&mut m, &nondet));
        assert_eq!((m.no_determinista, m.iguales, m.comparadas), (1, 0, 0));
        assert!(m.line().contains(" original=0 no_determinista=1 "), "{}", m.line());
        assert!(!contar_variante(&mut m, &Stats::default()));
        assert!(contar_variante(&mut m, &Stats { divergences: 1, ..Stats::default() }));
        assert!(contar_variante(&mut m, &Stats { known: 1, ..Stats::default() }));
        assert!(!contar_variante(&mut m, &Stats { inherited: 1, ..Stats::default() }));
        assert!(!contar_variante(&mut m, &Stats { runner_failed: 1, ..Stats::default() }));
        assert_eq!((m.iguales, m.divergencias, m.conocidas, m.heredadas, m.runner), (1, 1, 1, 1, 1));
        assert_eq!(m.comparadas, m.iguales + m.divergencias + m.conocidas + m.heredadas + (m.runner - m.runner_a));
        assert_eq!(m.no_determinista, 1);
        let mut t = MutStats::default();
        t.merge(&m);
        assert_eq!((t.no_determinista, t.comparadas), (1, 5));
    }

    /// M29: --emitir excluye corpus y --cruce.
    #[test]
    fn emitir_en_la_cli() {
        use super::Cli;
        let p = |v: &[&str]| Cli::parse(v.iter().map(|s| s.to_string()).collect());
        let c = p(&["--emitir", "x.js", "--minifier", "oxc", "--carril", "b"]).unwrap();
        assert_eq!(c.emitir.as_deref(), Some(std::path::Path::new("x.js")));
        assert!(p(&["--emitir", "x.js", "corpus"]).is_err());
        assert!(p(&["--emitir", "x.js", "--cruce", "ix"]).is_err());
        assert!(p(&["--emitir"]).is_err());
    }

    /// M34: minificar en un hijo es el default; `--en-proceso` lo apaga.
    #[test]
    fn subproceso_por_defecto() {
        use super::Cli;
        let p = |v: &[&str]| Cli::parse(v.iter().map(|s| s.to_string()).collect()).unwrap();
        assert!(p(&["c"]).subproceso);
        assert!(!p(&["c", "--en-proceso"]).subproceso);
    }

    #[test]
    fn la_celda_incluye_el_carril() {
        use super::Cli;
        let args = |extra: &[&str]| {
            let mut v = vec!["corpus".to_string()];
            v.extend(extra.iter().map(|s| s.to_string()));
            Cli::parse(v).unwrap()
        };
        // Carril A (default): el nombre de siempre (backend+preset+forma). B5: oxc por defecto.
        assert_eq!(args(&[]).cell(), "oxc+carril_a+tal_cual");
        assert_eq!(args(&["--carril", "a", "--config", "core_defaults"]).cell(), "oxc+core_defaults+tal_cual");
        assert_eq!(args(&["--carril", "b"]).cell(), "oxc+carril_a+tal_cual+carril_b");
        assert_eq!(args(&["--carril", "b", "--minifier", "swc"]).cell(), "swc+carril_a+tal_cual+carril_b");
        assert_eq!(
            args(&["--carril", "b", "--config", "core_defaults", "--forma", "en_funcion", "--minifier", "oxc"]).cell(),
            "oxc+core_defaults+en_funcion+carril_b"
        );
        assert!(Cli::parse(vec!["c".into(), "--carril".into(), "c".into()]).is_err());
    }
}
