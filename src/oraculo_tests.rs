//! R1-H05: el oraculo sin debug assertions, de punta a punta por `process_case`
//! (reporter, runner y node de verdad), con backends falsos: uno que hace panic
//! como un `debug_assert!` y un oraculo que minifica o que tambien falla.
//! Sin `node` en el PATH los tests de punta a punta avisan y salen.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::backend::Backend;
use crate::minify::{Config, Outcome};

/// Hace panic con cualquier fuente: el binario con debug assertions.
struct ConAsserts;
impl Backend for ConAsserts {
    fn prepare(&self, _: &str) -> Outcome {
        panic!("assertion failed: debug_assert simulado (compression_pass.rs)")
    }
}

/// El oraculo: devuelve el original tal cual y `minificado` como minificado.
struct Oraculo(&'static str);
impl Backend for Oraculo {
    fn prepare(&self, src: &str) -> Outcome {
        Outcome::Ok { original: src.to_string(), minified: self.0.to_string(), static_flags: Vec::new(), dead_writes: (0, 0) }
    }
}

/// El oraculo tambien falla: panic real.
struct TambienFalla;
impl Backend for TambienFalla {
    fn prepare(&self, _: &str) -> Outcome {
        Outcome::Panic("byte index 1 is out of range".into())
    }
}

fn hay_node() -> bool {
    std::process::Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Corre un caso (`console.log(1);`) por `process_case` con `cfg` y devuelve
/// las stats y el `--out`. El llamador borra el directorio.
fn correr(tag: &str, worker: usize, cfg: &Config) -> (crate::Stats, PathBuf) {
    let base = std::env::temp_dir().join(format!("difuzz-test-oraculo-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let corpus = base.join("corpus");
    let out = base.join("out");
    std::fs::create_dir_all(corpus.join("caso")).unwrap();
    std::fs::write(corpus.join("caso/input.js"), "console.log(1);\n").unwrap();
    let args = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "1"];
    let cli = crate::Cli::parse(args.iter().map(|s| s.to_string()).collect()).unwrap();
    let reporter = crate::oracle::Reporter::new(&out, cli.cell(), crate::runner::PRELOAD);
    let known = crate::known::Known::load(&corpus, &out, 0);
    let cache = crate::cache::CacheA::new(None, &[]);
    let triage = crate::triage::Triage::load(&out);
    let ctx = crate::Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, strict_recheck: None, oxc_recheck: None };
    let preload = crate::runner::Preload::write_tagged(&format!("oraculo-{tag}")).unwrap();
    let runner = crate::runner::Runner::new(&preload, worker, Duration::from_secs(10));
    let mut stats = crate::Stats::default();
    crate::process_case(&ctx, &runner, cfg, None, &cli, &mut stats, &corpus.join("caso/input.js"));
    (stats, base)
}

fn leer(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Panic solo con asserts, el oraculo minifica y diverge: div/ con la nota, nada en panic/.
#[test]
fn solo_asserts_se_compara_y_diverge() {
    if !hay_node() {
        eprintln!("sin node en el PATH: test salteado");
        return;
    }
    let cfg = Config::con_oraculo(Box::new(ConAsserts), Box::new(Oraculo("console.log(2);\n")));
    let (st, base) = correr("div", 9101, &cfg);
    let out = base.join("out");
    assert_eq!((st.panics, st.divergences, st.same, st.solo_asserts, st.solo_asserts_sin_div), (0, 1, 0, 1, 0));
    assert!(!out.join("panic").exists() && !out.join("asserts").exists());
    let meta = leer(&out.join("div/caso/meta.txt"));
    assert!(meta.contains("\nsolo_debug_assertions: si "), "{meta}");
    assert!(meta.contains("\npanic_con_debug_assertions:\nassertion failed: debug_assert simulado"), "{meta}");
    assert_eq!(leer(&out.join("div/caso/minified.cjs")), "console.log(2);\n");
    assert!(crate::minify::nota_asserts().is_none(), "la nota no sobrevive al caso");
    let _ = std::fs::remove_dir_all(base);
}

/// Panic solo con asserts que no diverge: no es hallazgo; queda en asserts/.
#[test]
fn solo_asserts_sin_divergencia_va_a_asserts() {
    if !hay_node() {
        eprintln!("sin node en el PATH: test salteado");
        return;
    }
    let cfg = Config::con_oraculo(Box::new(ConAsserts), Box::new(Oraculo("console.log(1)\n")));
    let (st, base) = correr("igual", 9102, &cfg);
    let out = base.join("out");
    assert_eq!((st.panics, st.divergences, st.same, st.solo_asserts, st.solo_asserts_sin_div), (0, 0, 1, 1, 1));
    assert!(!out.join("panic").exists() && !out.join("div").exists());
    let meta = leer(&out.join("asserts/caso/meta.txt"));
    assert!(meta.contains("kind: SoloDebugAssertions\n") && meta.contains("assertion failed: debug_assert simulado"), "{meta}");
    assert_eq!(leer(&out.join("asserts/caso/minified.cjs")), "console.log(1)\n", "el minificado del oraculo");
    let _ = std::fs::remove_dir_all(base);
}

/// Panic real (el oraculo tambien falla): panic/ como antes, con la nota.
#[test]
fn panic_real_sigue_en_panic() {
    let cfg = Config::con_oraculo(Box::new(ConAsserts), Box::new(TambienFalla));
    let (st, base) = correr("real", 9103, &cfg);
    let out = base.join("out");
    assert_eq!((st.panics, st.divergences, st.same, st.solo_asserts), (1, 0, 0, 0));
    let meta = leer(&out.join("panic/caso/meta.txt"));
    assert!(meta.contains("assertion failed: debug_assert simulado"), "{meta}");
    assert!(meta.contains("\noraculo sin debug assertions: tambien falla (es real):\nbyte index 1 is out of range"), "{meta}");
    assert!(!out.join("asserts").exists() && !out.join("div").exists());
    let _ = std::fs::remove_dir_all(base);
}
