//! R1-H09 y R2-H05 de punta a punta, con el binario: un directorio por celda
//! dentro de `--out` y la marca de corrida (`<out>/<celda>/CORRIDA.txt`).
//!
//! Cada test usa su TMPDIR propio (los temporales del runner y del hijo van
//! ahi) y lo borra al final. Sin node fallan, como el resto de los tests que
//! corren node (salvo `DIFUZZ_SIN_NODE=1`: ver src/test_util.rs).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// difuzz es solo binario: el helper comun se incluye por ruta.
#[allow(dead_code)]
#[path = "../src/test_util.rs"]
mod test_util;

const BIN: &str = env!("CARGO_BIN_EXE_difuzz");

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
    if !test_util::requiere_node() {
        return;
    }
    let b = base("celdas");
    let (corpus, out) = (b.join("corpus"), b.join("out"));
    // Depende de Function.prototype.toString: el minificado cambia el texto
    // (mangle de core_defaults). Va a known/ (flag fn_tostring) en los dos backends.
    caso(&corpus, "fts", "function f(primero) { var segundo = primero + 1; return segundo; }\nconsole.log(f.toString());\n");
    // Un descarte del original (11.2: antes `process.exit(3)`, descarte `exit`; ahora ese
    // original se compara): un require que no resuelve, descarte `enlace`.
    caso(&corpus, "sale", "require(\"./no-existe-difuzz-celdas\");\n");
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
        assert!(leer(&d.join("descartes.tsv")).contains("sale/input.js\tenlace\t"), "{}", d.display());
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
    if !test_util::requiere_node() {
        return;
    }
    let b = base("corte");
    let (corpus, out, idx) = (b.join("corpus"), b.join("out"), b.join("idx"));
    // Cada caso tarda ~1 s por ejecucion: con --jobs 1 la corrida dura >10 s.
    // Atomics.wait duerme 1 s de reloj real. No un bucle sobre Date.now(): el
    // preload congela Date (preload.js:27) y el bucle no termina nunca; tras el
    // SIGKILL de abajo ese node queda huerfano (pasa a init, nadie lo mata) y
    // gira al 100% de una CPU para siempre. MEDIDO 08-oct: 50 de esos node
    // acumulados por las corridas de la suite, ~39 CPU de carga, y los tests
    // con tiempos fallaban por la carga (b3-b, inestable/).
    for i in 0..6 {
        caso(
            &corpus,
            &format!("lento{i}"),
            "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1000);\nconsole.log('listo');\n",
        );
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
    // El indice (.tmp) se crea despues de la marca: esperarlo antes del SIGKILL
    // (MEDIDO b3-b: bajo carga 1,5 s no siempre alcanzaba).
    while !ix(".tmp").is_file() {
        assert!(t0.elapsed() < Duration::from_secs(60), "no aparecio el .tmp del indice");
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

/// 11.6 (I4): si difuzz muere de golpe (SIGKILL; o la segunda SIGTERM, que sale en el acto con
/// 130), el node que estaba corriendo un `while (true) {}` muere con el. Antes quedaba huerfano
/// (PPID 1) al 100 % de una CPU, sin timeout. Si el test falla, mata a los que encontro.
#[test]
fn segunda_senal_y_sigkill_no_dejan_node() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("huerfanos-node");
    let corpus = b.join("corpus");
    caso(&corpus, "gira", "while (true) {}\n");
    // Los node de la corrida `pid`: su preload esta en <tmp>/difuzz-<pid>-tmp/ (en su argv).
    let marca_tmp = |pid: u32| format!("{}/difuzz-{pid}-tmp/", b.join("tmp").display());
    let nodes_de = |pid: u32| -> Vec<i32> {
        let m = marca_tmp(pid);
        std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
            .filter(|p| std::fs::read(format!("/proc/{p}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains(&m)))
            .collect()
    };
    for modo in ["sigkill", "dos-sigterm"] {
        let out = b.join(format!("out-{modo}"));
        let args = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "1", "--timeout-ms", "120000"];
        let mut hijo = difuzz(&b, &args).spawn().unwrap();
        let pid = hijo.id();
        let t0 = Instant::now();
        while nodes_de(pid).is_empty() {
            assert!(t0.elapsed() < Duration::from_secs(60), "{modo}: el node de la corrida no arranco");
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: senales a un hijo propio vivo (sin cosechar).
        unsafe {
            if modo == "sigkill" {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            } else {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
                std::thread::sleep(Duration::from_millis(300));
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
        let st = hijo.wait().unwrap();
        if modo == "dos-sigterm" {
            assert_eq!(st.code(), Some(130), "{modo}");
        }
        let t0 = Instant::now();
        while !nodes_de(pid).is_empty() {
            if t0.elapsed() > Duration::from_secs(10) {
                let vivos = nodes_de(pid);
                for p in &vivos {
                    // SAFETY: kill a los node de esta corrida (cmdline con su directorio temporal).
                    unsafe { libc::kill(*p, libc::SIGKILL) };
                }
                panic!("{modo}: node {vivos:?} sobrevivio a difuzz (los mate)");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    std::fs::remove_dir_all(&b).unwrap();
}

/// 11.2d: cuatro B (bug F de oxc, oxc#27098) que pasan el tope de 300 MiB y despues giran
/// esperan, de a uno, su repeticion con los topes mayores (4 s cada una: el timeout largo, sin la
/// repeticion sin el sensor). Un SIGINT con la cola armada no la espera entera: la repeticion en curso
/// termina y los que esperaban quedan en descartes.tsv como `interrumpido`, sin destino inventado.
/// Antes (26c5239), unos 2 x 4 s por caso en cola (MEDIDO con c-cola de la revision B: 62,6 s).
#[test]
fn sigint_con_la_cola_de_topes_mayores_sale_enseguida() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("cola-sigint");
    let (corpus, out) = (b.join("corpus"), b.join("out"));
    for i in 0..4 {
        caso(
            &corpus,
            &format!("b-{i}"),
            &format!("const n = \"abc\".substring(1, undefined).length;\nlet z = 0;\nif (n === 0) {{ const b = []; for (let i = 0; i < 22; i++) b.push(Buffer.alloc(16 << 20, 1)); for (;;) z++; }}\nconsole.log(\"fin\", n, z, {i});\n"),
        );
    }
    let args = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "4", "--mem-node", "300", "--timeout-ms", "1000"];
    let marca = out.join("oxc+carril_a+tal_cual/CORRIDA.txt");
    let hijo = difuzz(&b, &args).spawn().unwrap();
    esperar_marca(&marca);
    // Los B mueren por el tope en menos de 1 s; a los 3 s la cola esta armada (16 s de repeticiones).
    std::thread::sleep(Duration::from_secs(3));
    let t0 = Instant::now();
    // SAFETY: kill a un pid propio (hijo vivo, no esperado todavia).
    assert_eq!(unsafe { libc::kill(hijo.id() as libc::pid_t, libc::SIGINT) }, 0);
    let o = hijo.wait_with_output().unwrap();
    let tardo = t0.elapsed();
    assert_eq!(o.status.code(), Some(130), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(tardo < Duration::from_secs(8), "tardo {tardo:?} en salir");
    let descartes = leer(&out.join("oxc+carril_a+tal_cual/descartes.tsv"));
    let n = descartes.lines().filter(|l| l.contains("\tinterrumpido\t")).count();
    assert!(n >= 1, "{descartes}");
    assert!(leer(&marca).contains(&format!("\ninterrumpidos: {n} ")), "{}", leer(&marca));
    std::fs::remove_dir_all(&b).unwrap();
}
