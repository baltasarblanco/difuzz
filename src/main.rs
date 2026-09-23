//! difuzz F1: esqueleto que camina (+ confianza: descartes, excepciones,
//! sensor de asunciones, canales, lint).
//!
//! GEN (corpus) → MIN (minify) → EXEC (runner) → CMP (oracle). Sin mutacion,
//! sin reduccion, una sola configuracion. Feo a proposito: el pipeline es la
//! entrega.

mod checks;
mod corpus;
mod known;
mod minify;
mod oracle;
mod runner;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use runner::Status;

const USAGE: &str = "\
uso: difuzz <corpus_dir> [--out DIR] [--config C] [--forma F] [--timeout-ms N] [--limit N] [--jobs N] [--no-observe]

  corpus_dir     raiz con archivos input.js (p. ej. crates/swc_ecma_minifier/tests)
  --out DIR      donde guardar hallazgos          (default: ./difuzz-out)
  --config C     carril_a | core_defaults           (default: carril_a)
  --forma F      tal_cual | en_funcion              (default: tal_cual)
  --timeout-ms N timeout por ejecucion de node    (default: 5000)
  --limit N      procesar solo los primeros N casos
  --jobs N       workers en paralelo                (default: nucleos disponibles)
  --no-observe   no agregar el epilogo observador a programas que no imprimen";

struct Cli {
    corpus: PathBuf,
    out: PathBuf,
    timeout: Duration,
    limit: Option<usize>,
    jobs: usize,
    observe: bool,
    preset: minify::Preset,
    forma: minify::Forma,
}

impl Cli {
    /// Celda de la matriz: nombra la salida y el oraculo (known-sintomas).
    fn cell(&self) -> String {
        format!("{}+{}", self.preset.name(), self.forma.name())
    }
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Cli, String> {
        let mut corpus = None;
        let mut out = PathBuf::from("difuzz-out");
        let mut timeout = Duration::from_millis(5000);
        let mut limit = None;
        let mut observe = true;
        let mut preset = minify::Preset::CarrilA;
        let mut forma = minify::Forma::TalCual;
        let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());

        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--out" => out = PathBuf::from(it.next().ok_or("--out necesita un valor")?),
                "--timeout-ms" => {
                    let v = it.next().ok_or("--timeout-ms necesita un valor")?;
                    timeout = Duration::from_millis(
                        v.parse().map_err(|_| format!("--timeout-ms invalido: {v}"))?,
                    );
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
                other if other.starts_with("--") => return Err(format!("opcion desconocida: {other}")),
                other => corpus = Some(PathBuf::from(other)),
            }
        }

        Ok(Cli {
            corpus: corpus.ok_or("falta <corpus_dir>")?,
            out,
            timeout,
            limit,
            jobs,
            observe,
            preset,
            forma,
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
    runner_failed: usize,
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
    }

    fn discard_original(&mut self, st: &Status) {
        match st {
            Status::Timeout => self.a_timeout += 1,
            Status::Signal => self.a_signal += 1,
            _ => self.a_exit += 1,
        }
    }

    fn line(&self) -> String {
        format!(
            "total={} iguales={} DIVERGENCIAS={} conocidas={} conocidas_sintoma_nuevo={} PANICS={} LINT={} observados={} observados_por_globals={} lanzaron={} | descartados: original={} (exit={} timeout={} senal={} invalido={} observado_sin_salida={} mudo_sin_observar={}) mudos_sin_bindings={} parse={} (esm={} otro={}) codegen={} runner={} ilegibles={} panics_del_fuzzer={}",
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
                + self.silent_no_observe,
            self.a_exit,
            self.a_timeout,
            self.a_signal,
            self.a_invalid,
            self.observed_not_ok,
            self.silent_no_observe,
            self.silent_unobservable,
            self.parse_esm + self.parse_other,
            self.parse_esm,
            self.parse_other,
            self.codegen_error,
            self.runner_failed,
            self.unreadable,
            self.fuzzer_panics,
        )
    }
}

fn main() {
    let cli = match Cli::parse(std::env::args().skip(1).collect()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    // Un panic del minifier es un hallazgo, no ruido: se captura en minify::prepare
    // y se guarda. El hook default lo imprimiria en stderr por cada caso.
    std::panic::set_hook(Box::new(|_| {}));

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
    let reporter = oracle::Reporter::new(&cli.out, cell.clone(), runner::PRELOAD);
    let known = known::Known::load(
        &cli.corpus,
        &cli.out,
        known::fnv1a(&[runner::PRELOAD, cell.as_str()]),
    );
    let ctx = Ctx {
        reporter: &reporter,
        known: &known,
    };
    let jobs = cli.jobs.min(files.len());

    eprintln!(
        "difuzz F1 · {} casos · config {} · {jobs} workers · timeout {:?} · observador {} · backlog conocido {} · salida en {}",
        files.len(),
        cell,
        cli.timeout,
        if cli.observe { "on" } else { "off" },
        known.backlog_len(),
        cli.out.display()
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
            .map(|w| {
                let (files, next, done, ctx, cli, preload) =
                    (&files, &next, &done, &ctx, &cli, &preload);
                scope.spawn(move || {
                    let cfg = cli.preset.build();
                    // Solo core_defaults cambia nombres: ahi cada divergencia se
                    // vuelve a probar con los nombres preservados (names_only).
                    let names_cfg = (cli.preset == minify::Preset::CoreDefaults)
                        .then(minify::Config::core_defaults_keep_names);
                    let runner = runner::Runner::new(preload, w, cli.timeout);
                    let mut stats = Stats::default();
                    loop {
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
                })
            })
            .collect();
        for w in workers {
            match w.join() {
                Ok(s) => stats.merge(&s),
                Err(_) => eprintln!("un worker murio por panic; sus casos no se cuentan"),
            }
        }
    });

    if let Err(e) = known.save() {
        eprintln!("no pude guardar known-sintomas.tsv: {e}");
    }
    print_hint(&cli.out, &stats);
    println!("{}", stats.line());
}

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
        minify::Outcome::ParseError { esm } => {
            if esm {
                stats.parse_esm += 1;
            } else {
                stats.parse_other += 1;
            }
            return;
        }
        minify::Outcome::CodegenError => {
            stats.codegen_error += 1;
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
        stats.lint += 1;
        eprintln!(
            "LINT escrituras sobre literales {} -> {}: {}",
            dead_writes.0,
            dead_writes.1,
            rel.display()
        );
        ctx.reporter.lint(rel, &original, &minified, dead_writes);
    }

    // Precondicion del oraculo: el original termina solo (exit 0 o lanzando).
    let a = runner.run(&original);
    // Un SyntaxError antes de imprimir nada puede ser que node no acepta el
    // original como script: entonces no es un caso.
    if a.threw() && a.stdout.starts_with("!dz throw SyntaxError") && !runner.node_accepts(&original) {
        stats.a_invalid += 1;
        return;
    }
    if let Status::Failed(e) = &a.status {
        stats.runner_failed += 1;
        eprintln!("runner: {e}");
        return;
    }

    if a.ok_with_output() {
        if a.threw() {
            stats.threw += 1;
        }
        if !compare_and_report(ctx, runner, stats, rel, &original, &minified, a, false, &static_flags, names_cfg) {
            stats.same += 1;
        }
    } else if a.ok_silent() {
        // Programa mudo. Primero la comparacion cruda: si el minificado lanza,
        // se cuelga o imprime algo, ya es divergencia (asi se ve un SyntaxError
        // como el de swc#12397 en un programa que no imprime). Los globals que
        // creo (lineas `!dz global`) entran en esta comparacion.
        let had_globals = a.has_globals();
        if compare_and_report(ctx, runner, stats, rel, &original, &minified, a, false, &static_flags, names_cfg) {
            return;
        }
        if !cli.observe {
            stats.silent_no_observe += 1;
            return;
        }
        // Segunda pasada con el epilogo observador.
        // El epilogo se agrega al fuente crudo y despues se aplica la forma: asi
        // queda dentro de la funcion y ve los bindings.
        let Some(instrumented) = minify::instrument(&raw).map(|s| cli.forma.apply(&s)) else {
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
        let a = runner.run(&original);
        if !a.ok_with_output() {
            stats.observed_not_ok += 1;
            return;
        }
        stats.observed += 1;
        if a.threw() {
            stats.threw += 1;
        }
        if !compare_and_report(ctx, runner, stats, rel, &original, &minified, a, true, &static_flags, names_cfg) {
            stats.same += 1;
        }
    } else {
        stats.discard_original(&a.status);
    }
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
    observed: bool,
    static_flags: &[&'static str],
    names_cfg: Option<&minify::Config>,
) -> bool {
    let b = runner.run(minified);
    if let Status::Failed(e) = &b.status {
        stats.runner_failed += 1;
        eprintln!("runner: {e}");
        return true;
    }
    match oracle::compare(&a, &b) {
        oracle::Verdict::Same => false,
        oracle::Verdict::Divergence(kind) => {
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
            let flags_b: Vec<&str> = b.flags.iter().map(String::as_str).collect();
            if names_cfg.is_some_and(|c| names_only(runner, c, original, &a)) {
                flags.push("nombres");
            }
            let mut why = ctx.known.classify(rel, &flags);
            // Una conocida que cambia de sintoma deja de estar cubierta.
            let drift = why.as_ref().and_then(|w| {
                let h = known::fnv1a(&[
                    format!("{kind:?}").as_str(),
                    oracle::normalize_names(&a.stdout).as_str(),
                    oracle::normalize_names(&b.stdout).as_str(),
                ]);
                ctx.known
                    .record_symptom(rel, h)
                    .err()
                    .map(|prev| format!("conocida ({w}) con sintoma nuevo: antes {prev:016x}, ahora {h:016x}"))
            });
            if drift.is_some() {
                why = None;
                stats.known_drift += 1;
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
                drift.as_deref(),
                &flags,
                &flags_b,
            );
            true
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
