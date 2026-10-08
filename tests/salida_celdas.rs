//! R1-H09 y R2-H05 de punta a punta, con el binario: un directorio por celda
//! dentro de `--out` y la marca de corrida (`<out>/<celda>/CORRIDA.txt`).
//!
//! Cada test usa su TMPDIR propio (los temporales del runner y del hijo van
//! ahi) y lo borra al final. Sin node no hay nada que correr: se saltean
//! avisando, como el resto de los tests que corren node.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_difuzz");

fn hay_node() -> bool {
    Command::new("node").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// `<tmp>/difuzz-test-<nombre>-<pid>/` vacio, con `tmp/` para TMPDIR.
fn base(nombre: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("difuzz-test-{nombre}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("tmp")).unwrap();
    d
}

fn caso(corpus: &Path, nombre: &str, src: &str) {
    let d = corpus.join(nombre);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("input.js"), src).unwrap();
}

fn difuzz(base: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(BIN);
    c.args(args)
        .env("TMPDIR", base.join("tmp"))
        // El oraculo sin debug assertions no hace falta aca (y puede no estar).
        .env("DIFUZZ_ORACULO", "ninguno")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

fn correr(base: &Path, args: &[&str]) -> std::process::Output {
    let o = difuzz(base, args).output().unwrap();
    assert!(
        o.status.success(),
        "difuzz {args:?} salio con {:?}\n{}",
        o.status,
        String::from_utf8_lossy(&o.stderr)
    );
    o
}

fn leer(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// R1-H09: dos celdas con el mismo `--out` conservan sus hallazgos, sus
/// descartes y su tabla de sintomas; otra corrida de una celda reemplaza los
/// suyos y no toca los de la otra.
#[test]
fn dos_celdas_mismo_out_no_se_pisan() {
    if !hay_node() {
        eprintln!("sin node en PATH: test omitido");
        return;
    }
    let b = base("celdas");
    let (corpus, out) = (b.join("corpus"), b.join("out"));
    // Depende de Function.prototype.toString: el minificado cambia el texto
    // (mangle de core_defaults). Va a known/ (flag fn_tostring) en los dos backends.
    caso(&corpus, "fts", "function f(primero) { var segundo = primero + 1; return segundo; }\nconsole.log(f.toString());\n");
    // Descarte por exit del original.
    caso(&corpus, "sale", "console.log(1);\nprocess.exit(3);\n");
    let o = out.to_str().unwrap();
    let c = corpus.to_str().unwrap();
    let celda = |m: &'static str| -> Vec<&str> {
        vec![c, "--out", o, "--minifier", m, "--config", "core_defaults", "--no-cache", "--jobs", "1"]
    };
    correr(&b, &celda("swc"));
    correr(&b, &celda("oxc"));
    let (swc, oxc) = (out.join("swc+core_defaults+tal_cual"), out.join("oxc+core_defaults+tal_cual"));
    for d in [&swc, &oxc] {
        assert!(d.join("known/fts/meta.txt").is_file(), "falta known/fts en {}", d.display());
        assert!(leer(&d.join("descartes.tsv")).contains("sale/input.js\texit\t"), "{}", d.display());
        assert!(leer(&d.join("known-sintomas.tsv")).contains("fts/input.js\t"), "{}", d.display());
        assert!(leer(&d.join("CORRIDA.txt")).starts_with("estado: completa\n"), "{}", d.display());
        // El repro del meta apunta al preload de la celda.
        assert!(d.join("preload.cjs").is_file());
    }
    // Nada en la raiz de --out salvo los directorios de celda.
    for n in ["div", "known", "descartes.tsv", "known-sintomas.tsv", "CORRIDA.txt"] {
        assert!(!out.join(n).exists(), "{n} en la raiz de --out");
    }

    // Restos de una corrida vieja de cada celda: solo los de la que se re-corre se van.
    for d in [&swc, &oxc] {
        std::fs::create_dir_all(d.join("div/viejo")).unwrap();
        std::fs::write(d.join("div/viejo/meta.txt"), "viejo").unwrap();
    }
    let sintomas_oxc = leer(&oxc.join("known-sintomas.tsv"));
    correr(&b, &celda("swc"));
    assert!(!swc.join("div/viejo").exists(), "la misma celda no reemplazo lo suyo");
    assert!(swc.join("known/fts/meta.txt").is_file());
    assert!(oxc.join("div/viejo/meta.txt").is_file(), "la otra celda perdio sus hallazgos");
    assert!(oxc.join("known/fts/meta.txt").is_file());
    assert_eq!(leer(&oxc.join("known-sintomas.tsv")), sintomas_oxc);
    assert_eq!(leer(&oxc.join("descartes.tsv")).lines().count(), 2);

    std::fs::remove_dir_all(&b).unwrap();
}

/// Espera a que exista la marca de corrida (el proceso ya arranco los
/// workers poco despues) o falla a los 60 s.
fn esperar_marca(p: &Path) {
    let t0 = Instant::now();
    while !p.is_file() {
        assert!(t0.elapsed() < Duration::from_secs(60), "no aparecio {}", p.display());
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// R2-H05: SIGINT a mitad de corpus deja `estado: interrumpida` (con cuantos
/// casos se procesaron) y el indice de la celda como `.incompleta`; SIGKILL no
/// corre nada y la marca queda en `incompleta`; la corrida siguiente de la
/// misma celda la lleva a `completa`.
#[test]
fn corrida_cortada_queda_marcada() {
    if !hay_node() {
        eprintln!("sin node en PATH: test omitido");
        return;
    }
    let b = base("corte");
    let (corpus, out, idx) = (b.join("corpus"), b.join("out"), b.join("idx"));
    // Cada caso tarda ~1 s por ejecucion: con --jobs 1 la corrida dura >10 s.
    for i in 0..6 {
        caso(&corpus, &format!("lento{i}"), "var t = Date.now();\nwhile (Date.now() - t < 1000);\nconsole.log('listo');\n");
    }
    let args = [
        corpus.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--indice",
        idx.to_str().unwrap(),
        "--minifier",
        "oxc",
        "--no-cache",
        "--jobs",
        "1",
    ];
    let celda = out.join("oxc+carril_a+tal_cual");
    let marca = celda.join("CORRIDA.txt");
    let ix = |suf: &str| idx.join(format!("index-oxc+carril_a+tal_cual.tsv{suf}"));

    // SIGINT
    let hijo = difuzz(&b, &args).spawn().unwrap();
    esperar_marca(&marca);
    assert!(leer(&marca).starts_with("estado: incompleta\n"));
    std::thread::sleep(Duration::from_millis(1500));
    // SAFETY: kill a un pid propio (hijo vivo, no esperado todavia).
    assert_eq!(unsafe { libc::kill(hijo.id() as libc::pid_t, libc::SIGINT) }, 0);
    let o = hijo.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(130), "{}", String::from_utf8_lossy(&o.stderr));
    let t = leer(&marca);
    assert!(t.starts_with("estado: interrumpida\n"), "{t}");
    assert!(t.contains("\ncelda: oxc+carril_a+tal_cual\n") && t.contains("\nfin: "), "{t}");
    let procesados = t.lines().find_map(|l| l.strip_prefix("procesados: ")).unwrap();
    assert!(procesados.ends_with(" de 6") && procesados != "6 de 6", "{procesados}");
    assert!(ix(".incompleta").is_file() && !ix("").exists() && !ix(".tmp").exists());

    // SIGKILL: sin handler. La marca queda como la dejo el arranque.
    let mut hijo = difuzz(&b, &args).spawn().unwrap();
    // La marca vieja (interrumpida) se reescribe al arrancar: esperar a que diga incompleta.
    let t0 = Instant::now();
    while !std::fs::read_to_string(&marca).is_ok_and(|t| t.starts_with("estado: incompleta\n")) {
        assert!(t0.elapsed() < Duration::from_secs(60), "la marca no volvio a incompleta");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(1500));
    hijo.kill().unwrap();
    let _ = hijo.wait();
    assert!(leer(&marca).starts_with("estado: incompleta\n"));
    assert!(ix(".tmp").is_file(), "SIGKILL deja el .tmp del indice");
    // Los node que quedaron huerfanos terminan solos (~1 s); el hijo minificador muere con el padre.
    std::thread::sleep(Duration::from_millis(1500));

    // La siguiente corrida de la celda avisa, termina y deja todo en completa.
    let o = correr(&b, &args);
    assert!(String::from_utf8_lossy(&o.stderr).contains("no termino (incompleta)"));
    assert!(leer(&marca).starts_with("estado: completa\n"));
    assert!(leer(&marca).contains("\nprocesados: 6 de 6\n"));
    assert!(ix("").is_file() && !ix(".incompleta").exists());
    let cruce = correr(&b, &["--cruce", idx.to_str().unwrap()]);
    assert!(!String::from_utf8_lossy(&cruce.stdout).contains("sin terminar"));

    std::fs::remove_dir_all(&b).unwrap();
}
