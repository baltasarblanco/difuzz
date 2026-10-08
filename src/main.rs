//! difuzz F1: esqueleto que camina (+ confianza: descartes, excepciones,
//! sensor de asunciones, canales, lint).
//!
//! GEN (corpus) → MIN (minify, backend swc|oxc) → EXEC (runner) → CMP (oracle).
//! Mutaciones opcionales (--mutaciones), sin reduccion. Feo a proposito: el
//! pipeline es la entrega.

mod backend;
mod cache;
mod checks;
mod corpus;
mod emitir;
mod huerfanos;
mod indice;
#[cfg(test)]
mod invocar_tests;
mod known;
mod minify;
mod mutate;
mod oracle;
mod recheck;
mod runner;
mod stubs;
mod subproceso;
mod triage;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use runner::Status;

const USAGE: &str = "\
uso: difuzz <corpus_dir> [--out DIR] [--minifier M] [--config C] [--forma F] [--carril K] [--timeout-ms N] [--limit N] [--jobs N] [--no-observe]
             [--invocar] [--mutaciones N] [--cache DIR | --no-cache] [--indice DIR] [--en-proceso]
       difuzz --cruce DIR
       difuzz --emitir ARCHIVO [--minifier M] [--config C] [--forma F] [--carril K]

  corpus_dir     raiz con archivos input.js (p. ej. crates/swc_ecma_minifier/tests)
  --out DIR      donde guardar hallazgos          (default: ./difuzz-out)
  --minifier M   swc | oxc                          (default: swc)
  --config C     carril_a | core_defaults           (default: carril_a)
  --forma F      tal_cual | en_funcion              (default: tal_cual)
  --carril K     a (script, .cjs) | b (modulo ES, .mjs) (default: a)
  --timeout-ms N timeout por ejecucion de node    (default: 5000)
  --limit N      procesar solo los primeros N casos
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
  --cache DIR    cache de la ejecucion del original, compartible entre celdas (default: <out>/cache-a)
  --no-cache     no leer ni escribir la cache
                 (cache_a_hits no es determinista: con la cache en frio depende de --jobs y del
                 orden en que los workers toman los casos; los resultados no cambian)
  --indice DIR   anotar cada hallazgo en DIR/index-<celda>.tsv (directorio compartido por las celdas)
  --cruce DIR    imprimir la tabla de cruce de DIR/index-*.tsv, sin correr nada
  --emitir ARCH  imprimir el original y el minificado de ARCH (sin correr node)
  --en-proceso   minificar en el proceso de difuzz. Por defecto se minifica en un hijo
                 persistente por worker: un abort, un desborde de pila o un cuelgue del
                 minifier queda en panic/ (con la senal) y la corrida sigue";

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
    kind: minify::SourceKind,
    /// M24: `--cruce DIR`: solo imprimir la tabla de cruce, sin correr nada.
    cruce: Option<PathBuf>,
    /// M34: minificar en un hijo (subproceso.rs). Por defecto; `--en-proceso` lo apaga.
    subproceso: bool,
    /// M29: `--emitir ARCHIVO`: imprimir original y minificado, sin correr nada.
    emitir: Option<PathBuf>,
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
        let mut minifier = backend::Minifier::Swc;
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
        let mut cruce = None;
        let mut subproceso = true;
        let mut emitir = None;

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
            kind,
            cruce,
            subproceso,
            emitir,
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
            "total={} iguales={} DIVERGENCIAS={} conocidas={} conocidas_sintoma_nuevo={} PANICS={} LINT={} observados={} observados_por_globals={} lanzaron={} | descartados: original={} (exit={} timeout={} senal={} invalido={} observado_sin_salida={} mudo_sin_observar={} enlace={} no_determinista={}) mudos_sin_bindings={} parse={} (esm={} otro={} script={}) codegen={} runner={} (a={} b={}) ilegibles={} panics_del_fuzzer={} | identicos={} lint_equivalentes={} cache_a_hits={}",
            self.total,
            self.same,
            self.divergences,
            self.known,
            self.known_drift,
            self.panics,
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
    let cli = match Cli::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    // `--cruce DIR`: solo lee los indices, no corre nada. Sin index-*.tsv: exit 1.
    if let Some(dir) = &cli.cruce {
        match indice::cruce(dir) {
            Ok(t) => print!("{t}"),
            Err(e) => {
                eprintln!("no pude leer los indices en {}: {e}", dir.display());
                std::process::exit(1);
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
                std::process::exit(1);
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
    let stale = runner::clean_stale();
    if stale > 0 {
        eprintln!("borre {stale} temporales de corridas anteriores cortadas (pid muerto)");
    }

    // M27 (d): "no existe", "no es directorio" y "sin input.js" son distintos.
    if let Err(e) = corpus::check_root(&cli.corpus) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    let files = corpus::collect(&cli.corpus);
    if files.is_empty() {
        eprintln!("no encontre ningun input.js bajo {}", cli.corpus.display());
        std::process::exit(1);
    }
    let files: Vec<PathBuf> = match cli.limit {
        Some(n) => files.into_iter().take(n).collect(),
        None => files,
    };

    let preload = match runner::Preload::write() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("no pude escribir el preload: {e}");
            std::process::exit(1);
        }
    };
    let cell = cli.cell();
    let errors_before = oracle::write_errors();
    let mut reporter = oracle::Reporter::new(&cli.out, cell.clone(), runner::PRELOAD)
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
    let cache = cache::CacheA::new(cli.cache.clone(), &cache_parts);
    // M27 (a): si --out no se puede escribir, no hay hallazgos que guardar.
    if oracle::write_errors() > errors_before {
        eprintln!("no puedo escribir en --out {}", cli.out.display());
        // exit no corre los Drop: el preload (y su directorio) se sueltan a mano.
        drop(preload);
        std::process::exit(1);
    }
    let triage = triage::Triage::load(&cli.out);
    let known = known::Known::load(
        &cli.corpus,
        &cli.out,
        // M4: "sintoma v2" (sin rutas por proceso): las tablas viejas se
        // descartan por el encabezado en vez de dar drift masivo.
        known::fnv1a(&[runner::PRELOAD, cell.as_str(), "sintoma v2"]),
    )
    .with_backend(cli.minifier);
    let ctx = Ctx {
        reporter: &reporter,
        known: &known,
        cache: &cache,
        triage: &triage,
        strict_recheck: (cli.minifier == backend::Minifier::Oxc && cli.kind == minify::SourceKind::Script)
            .then_some(cli.preset),
        oxc_recheck: (cli.minifier == backend::Minifier::Oxc).then_some((cli.preset, cli.kind)),
    };
    let jobs = cli.jobs.min(files.len());

    eprintln!(
        "difuzz F1 · {} casos · config {} · {jobs} workers · timeout {:?} · observador {} · backlog conocido {} · salida en {} · cache {}",
        files.len(),
        cell,
        cli.timeout,
        if cli.observe { "on" } else { "off" },
        known.backlog_len(),
        cli.out.display(),
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
                    let cfg = cli.preset.build(cli.minifier, cli.kind);
                    // Solo core_defaults cambia nombres: ahi cada divergencia se
                    // vuelve a probar con los nombres preservados (names_only).
                    let names_cfg = (cli.preset == minify::Preset::CoreDefaults)
                        .then(|| minify::Config::core_defaults_keep_names(cli.minifier, cli.kind));
                    let runner = runner::Runner::new(preload, w, cli.timeout)
                        .with_kind(cli.kind)
                        .with_invocar(cli.invocar)
                        .with_stubs(cli.stubs);
                    let mut stats = Stats::default();
                    loop {
                        if CANCEL.load(Ordering::SeqCst) {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = files.get(i) else {
                            break;
                        };
                        // Un panic nuestro (no del minifier: ese lo captura
                        // minify::prepare) pierde el caso, no el worker ni sus stats.
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            process_case(ctx, &runner, &cfg, names_cfg.as_ref(), cli, &mut stats, path)
                        }));
                        if r.is_err() {
                            stats.fuzzer_panics += 1;
                            eprintln!("panic del fuzzer en {}", path.display());
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
            std::process::exit(1);
        }
        for w in workers {
            match w.join() {
                Ok(s) => stats.merge(&s),
                Err(_) => eprintln!("un worker murio por panic; sus casos no se cuentan"),
            }
        }
    });

    let cancelled = CANCEL.load(Ordering::SeqCst);
    let late = huerfanos::cosechar();
    if late > 0 {
        eprintln!("subreaper: mate {late} procesos huerfanos al final de la corrida");
    }
    reporter.finish(!cancelled);
    if let Err(e) = known.save() {
        eprintln!("no pude guardar known-sintomas.tsv: {e}");
    }
    if let Err(e) = triage.save() {
        eprintln!("no pude guardar lint-triage.tsv: {e}");
    }
    print_hint(&cli.out, &stats);
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
    // M27 (a): una escritura perdida del reporter es un hallazgo perdido.
    let lost = oracle::write_errors() - errors_before;
    if lost > 0 && !cancelled {
        eprintln!("el reporter no pudo escribir {lost} archivos en {}: la corrida no es confiable", cli.out.display());
        drop(preload);
        std::process::exit(1);
    }
    if cancelled {
        eprintln!("corrida interrumpida: {} de {total} casos; el indice anterior quedo intacto", stats.total);
        drop(preload);
        std::process::exit(130);
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
        unsafe { libc::_exit(130) };
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
            return;
        }
    };
    let src = cli.forma.apply(&raw);

    let (original, minified, static_flags, dead_writes) = match minify::prepare(&src, cfg) {
        minify::Outcome::Ok {
            original,
            minified,
            static_flags,
            dead_writes,
        } => (original, minified, static_flags, dead_writes),
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
            stats.panics += 1;
            ctx.reporter.panic(rel, &src, &msg);
            return;
        }
    };

    // Lint sobre la pasada cruda (sin observador: el epilogo apagaria el
    // disparador). No depende de ejecutar nada.
    if dead_writes.1 > dead_writes.0 {
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
                ctx.reporter.lint(rel, &original, &minified, dead_writes, &note);
            }
        }
    }

    // M30: el manifiesto de stubs sale del original y vale para todo el caso.
    if cli.stubs {
        runner.set_stubs(&stubs::manifest(&original, cli.kind));
    }
    // Precondicion del oraculo: el original termina solo (exit 0 o lanzando).
    let (a, a_cached) = ctx.cache.run(runner, &original, &mut stats.cache_hits);
    // Un SyntaxError antes de imprimir nada puede ser que node no acepta el
    // original como script: entonces no es un caso.
    // R2-H03: tambien un RangeError que `node --check` reproduce: V8 no pudo
    // compilar el fuente (anidamiento extremo: 20000 `[`). Un RangeError en
    // runtime (recursion, `new Array(-1)`) pasa `node --check` y se compara.
    if a.threw()
        && (a.stdout.starts_with("!dz throw SyntaxError") || a.stdout.starts_with("!dz throw RangeError"))
        && !runner.node_accepts(&original)
    {
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

    // Mejora 10: variantes, solo si el original termina solo (si no, las
    // variantes casi seguro tampoco, y cada una costaria un timeout). M18: y no
    // si es mudo sin globals (las variantes se comparan solo por la pasada
    // cruda: serian mudas), y DESPUES de comparar el base, para no reportar una
    // variante que diverge igual que su base (dedup por kind).
    let mutate = cli.mutaciones + cli.mutaciones_extra > 0 && a.completed() && !(a.ok_silent() && !a.has_globals());
    let base_lint = dead_writes.1 > dead_writes.0;
    stats.last_div = None;
    compare_base(ctx, runner, cfg, names_cfg, cli, stats, rel, &raw, &original, &minified, &static_flags, a, a_cached);
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
    let Some(instrumented) = epilogo_normal(raw, cli) else {
        if had_globals {
            stats.observed_globals += 1;
            stats.same += 1;
        } else {
            stats.silent_unobservable += 1;
        }
        return;
    };
    let (original, minified, static_flags) = match minify::prepare(&instrumented, cfg) {
        minify::Outcome::Ok {
            original,
            minified,
            static_flags,
            ..
        } => (original, minified, static_flags),
        minify::Outcome::Panic(msg) => {
            stats.panics += 1;
            ctx.reporter.panic(rel, &instrumented, &msg);
            return;
        }
        // El fuente ya parseo sin el epilogo; si falla con el, es problema
        // del instrumentador, no un hallazgo.
        minify::Outcome::ParseError { .. } | minify::Outcome::CodegenError => {
            stats.silent_unobservable += 1;
            return;
        }
    };
    let (a, a_cached) = ctx.cache.run(runner, &original, &mut stats.cache_hits);
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
    stats.same += 1;
}

/// R2-H01: el epilogo de la pasada observada normal. No depende de `--invocar`:
/// es byte a byte el de una corrida sin `--invocar`.
fn epilogo_normal(raw: &str, cli: &Cli) -> Option<String> {
    minify::instrument(raw, false, cli.kind).map(|s| cli.forma.apply(&s))
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
    let Some(instrumented) = minify::instrument(raw, true, cli.kind).map(|s| cli.forma.apply(&s)) else {
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
            ..
        } => (original, minified, static_flags),
        minify::Outcome::Panic(msg) => {
            stats.panics += 1;
            ctx.reporter.panic(rel, &instrumented, &msg);
            return true;
        }
        minify::Outcome::ParseError { .. } | minify::Outcome::CodegenError => return false,
    };
    let (a, a_cached) = ctx.cache.run(runner, &original, &mut stats.cache_hits);
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

/// Precondicion de names_only: la salida del original puede mostrar nombres,
/// porque lee `.name` o imprime una instancia con el nombre de su clase
/// (`Foo { x: 7 }`). Sin esto, names_only tapaba bugs reales: keep_* tambien
/// apaga optimizaciones (inlining de funciones con nombre) y el 12188 dejaba de
/// dispararse.
fn names_visible(original: &str, a_stdout: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let reads_name = original
        .match_indices(".name")
        .any(|(i, _)| !original[i + 5..].chars().next().is_some_and(ident));
    let named_instance = a_stdout.lines().any(|l| {
        l.match_indices(" {")
            .any(|(i, _)| l[..i].chars().next_back().is_some_and(ident))
    });
    reads_name || named_instance
}

/// La divergencia desaparece si se minifica igual pero preservando nombres: es
/// el costo de keep_* apagado (`f.name`, `Foo { x: 7 }` impreso como `{ x: 7 }`),
/// no un bug. Se minifica el original ya impreso: es el mismo programa.
fn names_only(runner: &runner::Runner, cfg: &minify::Config, original: &str, a: &runner::Exec) -> bool {
    let minify::Outcome::Ok { minified, .. } = minify::prepare(original, cfg) else {
        return false;
    };
    matches!(oracle::compare(a, &runner.run(&minified)), oracle::Verdict::Same)
}

/// Flags cuya asuncion es sobre el contenido, no sobre la llamada.
const CONTENT_FLAGS: [&str; 2] = ["fn_tostring", "stack"];

/// Lo que comparten todas las comparaciones de una corrida.
struct Ctx<'a> {
    reporter: &'a oracle::Reporter,
    known: &'a known::Known,
    cache: &'a cache::CacheA,
    triage: &'a triage::Triage,
    /// M17: con `--minifier oxc --carril a`, el preset con el que se re-minifica
    /// el original en estricto ante una divergencia no conocida. `None` en las
    /// demas celdas.
    strict_recheck: Option<minify::Preset>,
    /// M13: con `--minifier oxc`, preset y carril para re-chequear el flag
    /// `extends` (ver `extends_matters`). `None` con swc.
    oxc_recheck: Option<(minify::Preset, minify::SourceKind)>,
}

/// M13: el flag `extends` (estatico) mira todo el programa: en 9460 un
/// `class extends 1` atrapado a proposito tapaba un bug real (O5). Se cambia
/// cada heredero literal por `null`, se minifica igual y se compara: si la
/// divergencia sigue, `extends` no la explica (`false`). Si no hay nada que
/// cambiar o algo falla, inconcluso (b1-b: antes `true`, se conservaba el flag).
/// El veredicto es el de recheck::veredicto (camino incluido).
fn extends_matters(
    runner: &runner::Runner,
    recheck: (minify::Preset, minify::SourceKind),
    original: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> recheck::Recheck {
    let (preset, kind) = recheck;
    let Some(neutral) = backend::swc::neutralize_extends(original, kind) else {
        return recheck::Recheck::Inconcluso("no pude neutralizar el heredero literal".into());
    };
    let cfg = preset.build(backend::Minifier::Oxc, kind);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&neutral, &cfg) else {
        return recheck::Recheck::Inconcluso("no pude minificar la variante con `extends null`".into());
    };
    let a2 = runner.run(&o2);
    if !a2.completed() {
        return recheck::Recheck::Inconcluso(format!("la variante con `extends null` no termino ({:?})", a2.status));
    }
    let b2 = runner.run(&m2);
    recheck::veredicto(a, b, &a2, &b2)
}

/// b1-b: aplica a `flags` el resultado del re-chequeo de `flag`. Solo `Explica`
/// lo deja; `NoExplica` lo cambia por `irrelevante` y `Inconcluso` por
/// `inconcluso` mas una nota para meta.txt.
fn apply_recheck(
    flags: &mut Vec<&str>,
    notes: &mut Vec<String>,
    flag: &str,
    (irrelevante, inconcluso): (&'static str, &'static str),
    r: recheck::Recheck,
) {
    let reemplazo = match r {
        recheck::Recheck::Explica => return,
        recheck::Recheck::NoExplica => irrelevante,
        recheck::Recheck::Inconcluso(n) => {
            notes.push(format!("re-chequeo de {flag} inconcluso: {n}"));
            inconcluso
        }
    };
    flags.retain(|f| *f != flag);
    flags.push(reemplazo);
}

/// b1-b (R2-H02, R1-H03): M16 concluyente. `None` si A no depende de la pila (o
/// la corrida honda fallo por el fuzzer). La profundidad explica la divergencia
/// solo si la corrida con mas pila termino sola y B difiere de A unicamente en
/// lo que tambien cambia con mas pila: B da lo mismo que A con mas pila, o las
/// tres salidas tienen las mismas lineas y cada linea en que B difiere de A es
/// una que cambia con la pila. Antes un Timeout de la corrida honda (o un bug en
/// otra linea) alcanzaba para mandar a known/.
fn depth_recheck(a: &runner::Exec, b: &runner::Exec, deep: &runner::Exec) -> Option<recheck::Recheck> {
    if matches!(deep.status, Status::Failed(_)) || same_exec(deep, a) {
        return None;
    }
    if !deep.completed() {
        return Some(recheck::Recheck::Inconcluso(format!(
            "la corrida con mas pila ({}) no termino ({:?})",
            runner::DEEP_STACK,
            deep.status
        )));
    }
    if same_exec(deep, b) || recheck::localizada(a, b, deep) {
        Some(recheck::Recheck::Explica)
    } else {
        Some(recheck::Recheck::Inconcluso(
            "el original depende de la pila, pero B difiere de A en lineas que no cambian con mas pila".into(),
        ))
    }
}

/// M17: oxc no soporta sloppy (D1 del CLAUDE.md). Ante una divergencia de oxc
/// en el carril A: si el original con `"use strict"` es valido para node y la
/// divergencia desaparece al minificarlo asi, es "solo sloppy" (`Ok(true)`). Si
/// no es estricto-valido, `Err(nota)`: queda en div/ para confirmar en el
/// carril B. Se minifica el original ya impreso: es el mismo programa.
/// b1-b (R1-H01): ademas el camino tiene que sostenerse (recheck::veredicto). Si
/// en estricto el original toma otra rama, la divergencia puede "desaparecer"
/// porque la rama con el bug no corre: eso es inconcluso (`Err`), no sloppy.
fn sloppy_only(
    runner: &runner::Runner,
    preset: minify::Preset,
    original: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> Result<bool, String> {
    let strict = format!("\"use strict\";\n{original}");
    if !runner.node_accepts(&strict) {
        return Err("no es estricto-valido: confirmar en el carril B".into());
    }
    let cfg = preset.build(backend::Minifier::Oxc, minify::SourceKind::Script);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&strict, &cfg) else {
        return Err("re-chequeo sloppy inconcluso: no pude minificar la variante estricta".into());
    };
    let a2 = runner.run(&o2);
    if !a2.completed() {
        return Err(format!("re-chequeo sloppy inconcluso: la variante estricta no termino ({:?})", a2.status));
    }
    let b2 = runner.run(&m2);
    match recheck::veredicto(a, b, &a2, &b2) {
        recheck::Recheck::Explica => Ok(true),
        recheck::Recheck::NoExplica => Ok(false),
        recheck::Recheck::Inconcluso(n) => Err(format!("re-chequeo sloppy inconcluso: {n}")),
    }
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
            // Una asuncion habla del programa de entrada: clasifican los flags de A
            // y los estaticos del original. Un flag solo en B es algo que el
            // minificado hace y el original no (p. ej. el .stack que Node lee al
            // cargar un SyntaxError): hallazgo, no asuncion.
            let mut flags: Vec<&str> = a.flags.iter().map(String::as_str).collect();
            flags.extend_from_slice(static_flags);
            // AGENTS.md habla de depender del *contenido* de toString y .stack, no
            // de llamarlos (`f++` llama a toString y da NaN igual). A se corre de
            // nuevo con esos contenidos cambiados: si la salida no cambia, no dependen.
            if flags.iter().any(|f| CONTENT_FLAGS.contains(f)) {
                let salted = runner.run_salted(original);
                if salted.status == a.status && salted.stdout == a.stdout {
                    flags.retain(|f| !CONTENT_FLAGS.contains(f));
                    flags.push("contenido_irrelevante");
                }
            }
            // M16: la salida puede depender del limite de pila de V8 y no del
            // programa (1000 funciones anidadas, una recursion que atrapa el
            // RangeError y devuelve cuantos frames entraron). Si A con el doble de
            // pila da otra salida, la divergencia depende de la profundidad del
            // motor. DESVIO de la propuesta: se prueba ante toda divergencia
            // confirmada, no solo si la salida dice "RangeError" (corpus-invocar/37
            // lo atrapa y no lo imprime). Cuesta una ejecucion por divergencia.
            // b1-b: notas de los re-chequeos inconclusos (van a meta.txt).
            let mut notes: Vec<String> = Vec::new();
            let deep = runner.run_with_args(original, &[runner::DEEP_STACK]);
            // b1-b (R2-H02): solo un M16 concluyente pone el flag (depth_recheck).
            match depth_recheck(&a, &b, &deep) {
                None => {}
                Some(recheck::Recheck::Explica) => flags.push("profundidad"),
                Some(r) => apply_recheck(&mut flags, &mut notes, "profundidad", ("profundidad_irrelevante", "profundidad_inconcluso"), r),
            }
            let flags_b: Vec<&str> = b.flags.iter().map(String::as_str).collect();
            if names_cfg.is_some_and(|c| names_visible(original, &a.stdout) && names_only(runner, c, original, &a)) {
                flags.push("nombres");
            }
            if flags.contains(&"extends") {
                if let Some(rc) = ctx.oxc_recheck {
                    let r = extends_matters(runner, rc, original, &a, &b);
                    apply_recheck(&mut flags, &mut notes, "extends", ("extends_irrelevante", "extends_inconcluso"), r);
                }
            }
            // Pendiente del fuego: `global_conocido` y `with` tambien se
            // re-chequean (recheck.rs), como `extends`. Solo oxc los clasifica.
            if let Some(rc) = ctx.oxc_recheck {
                if flags.contains(&"global_conocido") {
                    let r = recheck::globals_matter(runner, rc.1, original, minified, &a, &b);
                    apply_recheck(
                        &mut flags,
                        &mut notes,
                        "global_conocido",
                        ("global_conocido_irrelevante", "global_conocido_inconcluso"),
                        r,
                    );
                }
                if flags.contains(&"with") {
                    let r = recheck::with_matters(runner, rc, original, &a, &b);
                    apply_recheck(&mut flags, &mut notes, "with", ("with_irrelevante", "with_inconcluso"), r);
                }
            }
            let mut why = ctx.known.classify(rel, &flags);
            if let (None, Some(preset)) = (&why, ctx.strict_recheck) {
                match sloppy_only(runner, preset, original, &a, &b) {
                    Ok(true) => {
                        flags.push("solo_sloppy");
                        why = Some("oxc: solo sloppy (no soportado); con \"use strict\" la divergencia desaparece".into());
                    }
                    Ok(false) => {}
                    Err(nota) => notes.push(nota),
                }
            }
            // b1-b (R1-H03): un minificado que node no acepta nunca es asuncion:
            // ninguna permite que el minifier produzca codigo invalido. Se
            // clasifica sin flags (queda el backlog/ruta de SWC, que no son
            // asunciones). Solo se paga `node --check` si iba a known/.
            if why.is_some() && !runner.node_accepts(minified) && runner.node_accepts(original) {
                let sin_flags = ctx.known.classify(rel, &[]);
                if sin_flags.is_none() {
                    flags.push("minificado_invalido");
                    notes.push("node --check rechaza el minificado (y acepta el original): ninguna asuncion lo explica".into());
                }
                why = sin_flags;
            }
            // Una conocida que cambia de sintoma deja de estar cubierta.
            let drift = why.as_ref().and_then(|w| {
                let h = oracle::symptom(kind, &a, &b);
                ctx.known
                    .record_symptom(rel, h)
                    .err()
                    .map(|prev| format!("conocida ({w}) con sintoma nuevo: antes {prev:016x}, ahora {h:016x}"))
            });
            if drift.is_some() {
                why = None;
                stats.known_drift += 1;
            }
            // b1-b: el drift va primero (como antes, cuando pisaba la nota de sloppy).
            if let Some(d) = &drift {
                notes.insert(0, d.clone());
            }
            let nota = (!notes.is_empty()).then(|| notes.join(" | "));
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

/// Contadores de las variantes (mejora 10). `comparadas` = iguales + divergencias
/// + conocidas (+ fallas del runner en B).
#[derive(Default)]
struct MutStats {
    variantes: usize,
    comparadas: usize,
    iguales: usize,
    divergencias: usize,
    conocidas: usize,
    lint: usize,
    panics: usize,
    /// El original de la variante no imprime nada: sin observador, no hay que comparar.
    mudas: usize,
    /// El original de la variante no termina solo (timeout, exit, senal) o node no lo acepta.
    original: usize,
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
        self.mudas += o.mudas;
        self.original += o.original;
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
            "mutaciones: variantes={} comparadas={} iguales={} DIVERGENCIAS={} conocidas={} heredadas={} PANICS={} LINT={} | descartadas: mudas={} original={} parse={} runner={} (a={} b={}) | bases_sin_parse={}",
            self.variantes,
            self.comparadas,
            self.iguales,
            self.divergencias,
            self.conocidas,
            self.heredadas,
            self.panics,
            self.lint,
            self.mudas,
            self.original,
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
    let Some(vs) = mutate::variants_ext(raw, cli.mutaciones, cli.mutaciones_extra, known::fnv1a(&[raw]), cli.kind) else {
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
        let (original, minified, static_flags, dead_writes) = match minify::prepare(&src, cfg) {
            minify::Outcome::Ok {
                original,
                minified,
                static_flags,
                dead_writes,
            } => (original, minified, static_flags, dead_writes),
            minify::Outcome::Panic(msg) => {
                m.panics += 1;
                ctx.reporter.panic(&rel, &src, &msg);
                ctx.reporter.attach(&rel, &files);
                continue;
            }
            minify::Outcome::ParseError { .. } | minify::Outcome::CodegenError => {
                m.parse += 1;
                continue;
            }
        };
        // M18: si el base ya fue lint, el de la variante casi siempre es el mismo.
        if dead_writes.1 > dead_writes.0 && !base_lint {
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
                ctx.reporter.lint(&rel, &original, &minified, dead_writes, &note);
                ctx.reporter.attach(&rel, &files);
            }
        }
        if cli.stubs {
            runner.set_stubs(&stubs::manifest(&original, cli.kind));
        }
        let a = runner.run(&original);
        if let Status::Failed(e) = &a.status {
            m.runner += 1;
            m.runner_a += 1;
            eprintln!("runner: {e}");
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
        if !a.ok_with_output()
            || (a.stdout.starts_with("!dz throw SyntaxError") && !runner.node_accepts(&original))
        {
            m.original += 1;
            continue;
        }
        m.comparadas += 1;
        let mut s = Stats { skip_kind: base_div, ..Stats::default() };
        compare_and_report(ctx, runner, &mut s, &rel, &original, &minified, a, false, false, &static_flags, names_cfg);
        m.divergencias += s.divergences;
        m.conocidas += s.known;
        m.heredadas += s.inherited;
        m.runner += s.runner_failed;
        if s.inherited > 0 {
            continue;
        }
        if s.divergences + s.known > 0 {
            ctx.reporter.attach(&rel, &files);
        } else if s.runner_failed == 0 {
            m.iguales += 1;
        }
    }
}

fn print_hint(out: &Path, stats: &Stats) {
    if stats.divergences + stats.panics > 0 {
        eprintln!("hallazgos en {}/div", out.display());
    }
    if stats.known > 0 {
        eprintln!("conocidas (asuncion o backlog) en {}/known", out.display());
    }
    if stats.lint > 0 {
        eprintln!("sospechosos del lint (triage a mano) en {}/lint", out.display());
    }
}

#[cfg(test)]
mod tests {
    use super::names_visible;

    #[test]
    fn nombres_visibles_solo_si_la_salida_puede_mostrarlos() {
        assert!(names_visible("console.log(f.name);", ""));
        assert!(names_visible("", "Foo { x: 7 }"));
        assert!(names_visible("", "A {} A {}"));
        // 12188: nada de nombres a la vista.
        assert!(!names_visible("a[index++]=1;", "member result 1 1"));
        assert!(!names_visible("o.names;", "x = { a: 1 }"));
        assert!(!names_visible("", "Map(1) { 'a' => 1 }"));
    }

    #[test]
    fn la_celda_incluye_el_backend() {
        let cli = |args: &[&str]| super::Cli::parse(args.iter().map(|s| s.to_string()).collect()).unwrap();
        assert_eq!(cli(&["corpus"]).cell(), "swc+carril_a+tal_cual");
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
        let d = std::env::temp_dir().join(format!("difuzz-test-identico-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx {
            reporter: &reporter,
            known: &known,
            cache: &cache,
            triage: &triage,
            strict_recheck: None,
            oxc_recheck: None,
        };
        let preload = runner::Preload::write_tagged("identico").unwrap();
        let r = runner::Runner::new(&preload, 999, std::time::Duration::from_secs(5));
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
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-invalido-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0).with_backend(Minifier::Oxc);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, strict_recheck: None, oxc_recheck: None };
        let preload = runner::Preload::write_tagged("invalido").unwrap();
        let r = runner::Runner::new(&preload, 989, std::time::Duration::from_secs(20));
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
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// M17: "solo sloppy" exige que el original sea estricto-valido y que la
    /// divergencia desaparezca en estricto. Un bug real (O1) no se esconde.
    #[test]
    fn oxc_solo_sloppy() {
        use super::{backend::Minifier, minify, minify::Preset, runner, sloppy_only};
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let preload = runner::Preload::write_tagged("m17").unwrap();
        let r = runner::Runner::new(&preload, 998, std::time::Duration::from_secs(20));
        // b1-b: A y B de verdad (oxc core_defaults), no solo la variante estricta.
        let ab = |src: &str| {
            let cfg = Preset::CoreDefaults.build(Minifier::Oxc, minify::SourceKind::Script);
            let minify::Outcome::Ok { original, minified, .. } = minify::prepare(src, &cfg) else {
                panic!("no minifica: {src}");
            };
            let (a, b) = (r.run(&original), r.run(&minified));
            (original, a, b)
        };
        // terser/compress/dead_code/dead_code_2_should_warn (corpus completo): `g`
        // es una funcion en bloque (Anexo B, solo sloppy). En sloppy `g()` da
        // TypeError, en estricto ReferenceError: mismo punto, se acepta como sloppy.
        let (o, a, b) = ab("function f(){g();x=10;throw new Error(`foo`);if(x){y();var x;function g(){}(function(){var q;function y(){}})()}}f();\n");
        assert_eq!(a.stdout, "!dz throw TypeError\n");
        assert_eq!(sloppy_only(&r, Preset::CoreDefaults, &o, &a, &b), Ok(true));
        let with = "with ({}) { console.log(1); }\n";
        assert!(sloppy_only(&r, Preset::CarrilA, with, &r.run(with), &r.run(with)).is_err());
        let (o, a, b) = ab("const __proto__ = 0;\nconsole.log(Object.keys({ __proto__ }));\n");
        assert_eq!(sloppy_only(&r, Preset::CoreDefaults, &o, &a, &b), Ok(false));
        // b1-b (R1-H01, c4 10-rt-sloppy-miente): el bug de oxc (borra la IIFE con
        // rest y default con efecto) esta en la rama sloppy. En estricto el
        // original toma la otra rama: antes `Ok(true)` (known/), ahora inconcluso.
        let (o, a, b) = ab("function rec(v) { console.log(v); }\nfunction f() { return this; }\nif (f() === undefined) {\n  console.log(\"estricto\");\n} else {\n  (function (...[value = rec(\"rest\")]) {})();\n}\nconsole.log(\"fin\");\n");
        assert_eq!(a.stdout, "rest\nfin\n");
        assert_ne!(b.stdout, a.stdout, "oxc ya no tiene el bug de 9460: buscar otro");
        let s = sloppy_only(&r, Preset::CoreDefaults, &o, &a, &b);
        assert!(s.as_ref().is_err_and(|n| n.contains("otro camino")), "{s:?}");
    }

    /// b1-b (R2-H02): M16 solo es concluyente si la corrida honda termina y B
    /// difiere de A solo en lineas que dependen de la pila.
    #[test]
    fn profundidad_concluyente() {
        use super::{depth_recheck, recheck::Recheck, runner::{Exec, Status}};
        let ex = |status: Status, s: &str| Exec {
            status,
            stdout: s.into(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: std::time::Duration::ZERO,
            ctl: String::new(),
        };
        let ok = |s: &str| ex(Status::Exited(0), s);
        // A no depende de la pila: no hay flag.
        assert_eq!(depth_recheck(&ok("1\nx\n"), &ok("2\nx\n"), &ok("1\nx\n")), None);
        // Solo cambia la linea que depende de la pila: concluyente.
        assert_eq!(depth_recheck(&ok("1200\nx\n"), &ok("1500\nx\n"), &ok("2400\nx\n")), Some(Recheck::Explica));
        // B da lo mismo que A con mas pila (A se quedo sin pila, B no).
        assert_eq!(depth_recheck(&ok("corto\n"), &ok("largo\n"), &ok("largo\n")), Some(Recheck::Explica));
        // 07-profundidad-miente: ademas de la linea de la pila, B pierde otra.
        let r = depth_recheck(&ok("playo 3\n[ '__proto__' ]\n"), &ok("playo 5\n[]\n"), &ok("hondo 1\n[ '__proto__' ]\n"));
        assert!(matches!(r, Some(Recheck::Inconcluso(_))), "{r:?}");
        // R2-H02: la corrida honda en Timeout no es evidencia de profundidad.
        let r = depth_recheck(&ok("1\n"), &ok("2\n"), &ex(Status::Timeout, ""));
        assert!(matches!(r, Some(Recheck::Inconcluso(ref n)) if n.contains("Timeout")), "{r:?}");
        // Un Failed del fuzzer sigue sin opinar.
        assert_eq!(depth_recheck(&ok("1\n"), &ok("2\n"), &ex(Status::Failed("x".into()), "")), None);
    }

    /// M31 (a): si invocar no termina (cuelgue, exit propio), la observacion se
    /// repite sin invocar en vez de perder el caso.
    #[test]
    fn invocar_que_no_termina_reintenta_sin_invocar() {
        use super::{backend::Minifier, cache, known, minify, observe_epilogue, oracle, runner, triage, Cli, Ctx, Stats};
        use std::path::Path;
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-m31a-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD);
        let known = known::Known::load(&d, &d, 0);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, strict_recheck: None, oxc_recheck: None };
        let cli = Cli::parse(["c", "--invocar", "--no-cache"].iter().map(|s| s.to_string()).collect()).unwrap();
        let cfg = minify::Preset::CarrilA.build(Minifier::Swc, minify::SourceKind::Script);
        let preload = runner::Preload::write_tagged("m31a").unwrap();
        let r = runner::Runner::new(&preload, 990, std::time::Duration::from_millis(1500));
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
                    let normal = epilogo_normal(raw, &con);
                    assert_eq!(normal, epilogo_normal(raw, &sin), "{carril} {forma} {raw}");
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

    /// R2-H03: un original que V8 no puede compilar (RangeError al parsear
    /// 6000 `[` anidados; MEDIDO con node 24: 3000 compila, 3500 no; `node --check` lo rechaza igual) es descarte
    /// `invalido` en los dos backends y carriles, no un ThrowLost en div/. Un
    /// RangeError en runtime pasa `node --check` y se sigue comparando.
    #[test]
    fn original_que_node_no_compila_es_invalido() {
        use super::{backend::Minifier, cache, known, minify, oracle, process_case, runner, triage, Cli, Ctx, Stats, WORKER_STACK};
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
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
                let known = known::Known::load(&d, &d, 0);
                for (m, minifier) in [("swc", Minifier::Swc), ("oxc", Minifier::Oxc)] {
                    for (k, kind) in [("a", minify::SourceKind::Script), ("b", minify::SourceKind::Module)] {
                        let out = d.join(format!("out-{m}-{k}"));
                        std::fs::create_dir_all(&out).unwrap();
                        let reporter = oracle::Reporter::new(&out, "test".into(), runner::PRELOAD).with_kind(kind);
                        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, strict_recheck: None, oxc_recheck: None };
                        let cli = Cli::parse(
                            [corpus.to_str().unwrap(), "--minifier", m, "--carril", k, "--no-cache"].iter().map(|s| s.to_string()).collect(),
                        )
                        .unwrap();
                        let cfg = minify::Preset::CarrilA.build(minifier, kind);
                        let r = runner::Runner::new(&preload, 992, std::time::Duration::from_secs(10)).with_kind(kind);
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

    /// R2-H01, de punta a punta: oxc/B con `--invocar` detecta el patron de
    /// 6422/2 (spread con getter que el inlining corre despues del assert), que
    /// la re-observacion de M31 (c) tapaba cuando reemplazaba a la normal.
    #[test]
    fn invocar_no_pierde_el_bug_de_inlining() {
        use super::{backend::Minifier, cache, known, minify, observe_epilogue, oracle, runner, triage, Cli, Ctx, Stats};
        use std::path::Path;
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let d = std::env::temp_dir().join(format!("difuzz-test-h01-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let reporter = oracle::Reporter::new(&d, "test".into(), runner::PRELOAD).with_kind(minify::SourceKind::Module);
        let known = known::Known::load(&d, &d, 0);
        let cache = cache::CacheA::new(None, &[]);
        let triage = triage::Triage::load(&d);
        let ctx = Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, strict_recheck: None, oxc_recheck: None };
        let cfg = minify::Preset::CoreDefaults.build(Minifier::Oxc, minify::SourceKind::Module);
        let preload = runner::Preload::write_tagged("h01").unwrap();
        let r = runner::Runner::new(&preload, 991, std::time::Duration::from_secs(10)).with_kind(minify::SourceKind::Module);
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
        // Carril A (default): el nombre de siempre (backend+preset+forma).
        assert_eq!(args(&[]).cell(), "swc+carril_a+tal_cual");
        assert_eq!(args(&["--carril", "a", "--config", "core_defaults"]).cell(), "swc+core_defaults+tal_cual");
        assert_eq!(args(&["--carril", "b"]).cell(), "swc+carril_a+tal_cual+carril_b");
        assert_eq!(
            args(&["--carril", "b", "--config", "core_defaults", "--forma", "en_funcion", "--minifier", "oxc"]).cell(),
            "oxc+core_defaults+en_funcion+carril_b"
        );
        assert!(Cli::parse(vec!["c".into(), "--carril".into(), "c".into()]).is_err());
    }
}
