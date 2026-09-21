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
uso: difuzz <corpus_dir> [--out DIR] [--timeout-ms N] [--limit N]

  corpus_dir     raiz con archivos input.js (p. ej. crates/swc_ecma_minifier/tests)
  --out DIR      donde guardar hallazgos          (default: ./difuzz-out)
  --timeout-ms N timeout por ejecucion de node    (default: 5000)
  --limit N      procesar solo los primeros N casos";

struct Cli {
    corpus: PathBuf,
    out: PathBuf,
    timeout: Duration,
    limit: Option<usize>,
}

impl Cli {
    fn parse(args: Vec<String>) -> Result<Cli, String> {
        let mut corpus = None;
        let mut out = PathBuf::from("difuzz-out");
        let mut timeout = Duration::from_millis(5000);
        let mut limit = None;

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
                other if other.starts_with("--") => return Err(format!("opcion desconocida: {other}")),
                other => corpus = Some(PathBuf::from(other)),
            }
        }

        Ok(Cli {
            corpus: corpus.ok_or("falta <corpus_dir>")?,
            out,
            timeout,
            limit,
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
}

impl Stats {
    fn line(&self) -> String {
        format!(
            "total={} iguales={} DIVERGENCIAS={} PANICS={} | descartados: original={} parse={} codegen={} runner={} ilegibles={}",
            self.total,
            self.same,
            self.divergences,
            self.panics,
            self.original_discarded,
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
    let reporter = oracle::Reporter::new(&cli.out, cfg.name);
    let mut stats = Stats::default();

    eprintln!(
        "difuzz F1 · {} casos · config {} · timeout {:?} · salida en {}",
        files.len(),
        cfg.name,
        cli.timeout,
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
        let a = runner::run(&original, cli.timeout);
        if let Status::Failed(e) = &a.status {
            stats.runner_failed += 1;
            eprintln!("runner: {e}");
            continue;
        }
        if !a.ok_with_output() {
            stats.original_discarded += 1;
            continue;
        }

        let b = runner::run(&minified, cli.timeout);
        if let Status::Failed(e) = &b.status {
            stats.runner_failed += 1;
            eprintln!("runner: {e}");
            continue;
        }

        match oracle::compare(&a, &b) {
            oracle::Verdict::Same => stats.same += 1,
            oracle::Verdict::Divergence(kind) => {
                stats.divergences += 1;
                eprintln!("DIVERGENCIA {kind:?}: {}", rel.display());
                reporter.divergence(rel, &original, &minified, &a, &b, kind);
            }
        }

        if (i + 1) % 100 == 0 {
            eprintln!("[{}/{}] {}", i + 1, files.len(), stats.line());
        }
    }

    println!("{}", stats.line());
    print_hint(&cli.out, &stats);
}

fn print_hint(out: &Path, stats: &Stats) {
    if stats.divergences + stats.panics > 0 {
        println!("hallazgos en {}", out.display());
    }
}
