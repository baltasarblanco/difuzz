//! difuzz F1: esqueleto que camina.
//!
//! GEN (corpus) → MIN (minify) → EXEC (runner) → CMP (oracle). Sin mutacion,
//! sin reduccion, una sola configuracion. Feo a proposito: el pipeline es la
//! entrega.

mod corpus;
mod minify;
mod oracle;
mod runner;

use std::path::{Path, PathBuf};
use std::time::Duration;

use runner::Status;

const USAGE: &str = "\
uso: difuzz <corpus_dir> [--out DIR] [--timeout-ms N] [--limit N] [--no-observe]

  corpus_dir     raiz con archivos input.js (p. ej. crates/swc_ecma_minifier/tests)
  --out DIR      donde guardar hallazgos          (default: ./difuzz-out)
  --timeout-ms N timeout por ejecucion de node    (default: 5000)
  --limit N      procesar solo los primeros N casos
  --no-observe   no agregar el epilogo observador a programas que no imprimen";

struct Cli {
    corpus: PathBuf,
    out: PathBuf,
    timeout: Duration,
    limit: Option<usize>,
    observe: bool,
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Cli, String> {
        let mut corpus = None;
        let mut out = PathBuf::from("difuzz-out");
        let mut timeout = Duration::from_millis(5000);
        let mut limit = None;
        let mut observe = true;

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
                "--no-observe" => observe = false,
                other if other.starts_with("--") => return Err(format!("opcion desconocida: {other}")),
                other => corpus = Some(PathBuf::from(other)),
            }
        }

        Ok(Cli {
            corpus: corpus.ok_or("falta <corpus_dir>")?,
            out,
            timeout,
            limit,
            observe,
        })
    }
}

#[derive(Default)]
struct Stats {
    total: usize,
    unreadable: usize,
    original_discarded: usize,
    parse_error: usize,
    codegen_error: usize,
    runner_failed: usize,
    panics: usize,
    same: usize,
    divergences: usize,
    /// Casos mudos comparados via epilogo (subconjunto de same + divergences).
    observed: usize,
    /// Casos mudos sin bindings top-level que observar.
    silent_unobservable: usize,
}

impl Stats {
    fn line(&self) -> String {
        format!(
            "total={} iguales={} DIVERGENCIAS={} PANICS={} observados={} | descartados: original={} mudos_sin_bindings={} parse={} codegen={} runner={} ilegibles={}",
            self.total,
            self.same,
            self.divergences,
            self.panics,
            self.observed,
            self.original_discarded,
            self.silent_unobservable,
            self.parse_error,
            self.codegen_error,
            self.runner_failed,
            self.unreadable,
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

    // La config se construye una sola vez: MangleOptions::default() pasa por
    // serde_json (delta v1.2 §1 §3.5).
    let cfg = minify::Config::carril_a();
    let runner = match runner::Runner::new(cli.timeout) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no pude preparar el runner: {e}");
            std::process::exit(1);
        }
    };
    let reporter = oracle::Reporter::new(&cli.out, cfg.name, runner::PRELOAD);
    let mut stats = Stats::default();

    eprintln!(
        "difuzz F1 · {} casos · config {} · timeout {:?} · observador {} · salida en {}",
        files.len(),
        cfg.name,
        cli.timeout,
        if cli.observe { "on" } else { "off" },
        cli.out.display()
    );

    for (i, path) in files.iter().enumerate() {
        stats.total += 1;
        let rel = path.strip_prefix(&cli.corpus).unwrap_or(path.as_path());

        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => {
                stats.unreadable += 1;
                continue;
            }
        };

        let (original, minified) = match minify::prepare(&src, &cfg) {
            minify::Outcome::Ok { original, minified } => (original, minified),
            minify::Outcome::ParseError(_) => {
                stats.parse_error += 1;
                continue;
            }
            minify::Outcome::CodegenError(_) => {
                stats.codegen_error += 1;
                continue;
            }
            minify::Outcome::Panic(msg) => {
                stats.panics += 1;
                reporter.panic(rel, &src, &msg);
                continue;
            }
        };

        // Precondicion del oraculo: el original corre y dice algo.
        let a = runner.run(&original);
        if let Status::Failed(e) = &a.status {
            stats.runner_failed += 1;
            eprintln!("runner: {e}");
            continue;
        }

        if a.ok_with_output() {
            if !compare_and_report(&runner, &reporter, &mut stats, rel, &original, &minified, a, false) {
                stats.same += 1;
            }
        } else if a.ok_silent() {
            // Programa mudo. Primero la comparacion cruda: si el minificado lanza,
            // se cuelga o imprime algo, ya es divergencia (asi se ve un SyntaxError
            // como el de swc#12397 en un programa que no imprime).
            if compare_and_report(&runner, &reporter, &mut stats, rel, &original, &minified, a, false) {
                continue;
            }
            if !cli.observe {
                stats.original_discarded += 1;
                continue;
            }
            // Segunda pasada con el epilogo observador.
            let Some(instrumented) = minify::instrument(&src) else {
                stats.silent_unobservable += 1;
                continue;
            };
            let (original, minified) = match minify::prepare(&instrumented, &cfg) {
                minify::Outcome::Ok { original, minified } => (original, minified),
                minify::Outcome::Panic(msg) => {
                    stats.panics += 1;
                    reporter.panic(rel, &instrumented, &msg);
                    continue;
                }
                // El fuente ya parseo sin el epilogo; si falla con el, es problema
                // del instrumentador, no un hallazgo.
                minify::Outcome::ParseError(_) | minify::Outcome::CodegenError(_) => {
                    stats.silent_unobservable += 1;
                    continue;
                }
            };
            let a = runner.run(&original);
            if !a.ok_with_output() {
                stats.original_discarded += 1;
                continue;
            }
            stats.observed += 1;
            if !compare_and_report(&runner, &reporter, &mut stats, rel, &original, &minified, a, true) {
                stats.same += 1;
            }
        } else {
            stats.original_discarded += 1;
        }

        if (i + 1) % 100 == 0 {
            eprintln!("[{}/{}] {}", i + 1, files.len(), stats.line());
        }
    }

    println!("{}", stats.line());
    print_hint(&cli.out, &stats);
}

/// Corre B y compara contra A (que ya termino con exit 0). Devuelve `true` si
/// el caso quedo cerrado: divergencia reportada o falla del runner. `false`
/// significa "iguales"; el llamador decide si eso cuenta como `same` o si sigue
/// a la pasada observada.
#[allow(clippy::too_many_arguments)]
fn compare_and_report(
    runner: &runner::Runner,
    reporter: &oracle::Reporter,
    stats: &mut Stats,
    rel: &Path,
    original: &str,
    minified: &str,
    a: runner::Exec,
    observed: bool,
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
            stats.divergences += 1;
            let tag = if observed { " (observado)" } else { "" };
            eprintln!("DIVERGENCIA {kind:?}{tag}: {}", rel.display());
            reporter.divergence(rel, original, minified, &a, &b, kind, observed);
            true
        }
    }
}

fn print_hint(out: &Path, stats: &Stats) {
    if stats.divergences + stats.panics > 0 {
        println!("hallazgos en {}", out.display());
    }
}
