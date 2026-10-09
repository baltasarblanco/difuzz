//! b6-a: lo que ve el usuario, con el binario. R2-H09 (codigos de salida de
//! la tabla del USAGE, un test por codigo; el 130 lo prueba
//! `salida_celdas::corrida_cortada_queda_marcada`) y R2-H06 (descartes.tsv
//! permite reconstruir `total=` caso por caso).
//!
//! Cada test usa su TMPDIR propio y lo borra al final. Los que corren node
//! fallan sin node, salvo `DIFUZZ_SIN_NODE=1` (src/test_util.rs), como el resto.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_difuzz");

// R1-H17: el mismo `requiere_node()` que el resto de los tests (difuzz es solo binario).
#[allow(dead_code)]
#[path = "../src/test_util.rs"]
mod test_util;

/// `<tmp>/difuzz-test-b6-<nombre>-<pid>/` vacio, con `tmp/` para TMPDIR.
fn base(nombre: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("difuzz-test-b6-{nombre}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("tmp")).unwrap();
    d
}

fn caso(corpus: &Path, nombre: &str, src: &[u8]) {
    let d = corpus.join(nombre);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("input.js"), src).unwrap();
}

fn difuzz(base: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("TMPDIR", base.join("tmp"))
        .env("DIFUZZ_ORACULO", "ninguno")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn rc(o: &Output) -> Option<i32> {
    o.status.code()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// 0: `--help` imprime la ayuda completa en stdout (antes: "opcion
/// desconocida" y 2), y una corrida entera sale con 0.
#[test]
fn codigo_0_ayuda_y_corrida_entera() {
    let b = base("rc0");
    for h in ["--help", "-h"] {
        let o = difuzz(&b, &[h]);
        assert_eq!(rc(&o), Some(0), "{h}: {}", err(&o));
        let s = String::from_utf8_lossy(&o.stdout);
        let uso: String = s.lines().take_while(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n");
        // R2-H09: la linea `uso:` tiene todas las opciones que acepta Cli::parse.
        for opt in [
            "--out", "--minifier", "--config", "--forma", "--carril", "--timeout-ms", "--limit", "--jobs", "--no-observe",
            "--invocar", "--mutaciones N", "--mutaciones-extra", "--stubs", "--cache", "--no-cache", "--indice",
            "--en-proceso", "--mem-hijo", "--cruce", "--emitir", "--help",
        ] {
            assert!(uso.contains(opt), "falta {opt} en la linea uso:\n{uso}");
        }
        assert!(s.contains("codigos de salida"), "{s}");
        assert!(s.contains("total = iguales + DIVERGENCIAS + conocidas + PANICS + RECURSOS + filas(descartes.tsv)"), "{s}");
    }
    // La corrida entera necesita node: sin node falla (salvo DIFUZZ_SIN_NODE=1).
    if test_util::requiere_node() {
        let corpus = b.join("corpus");
        caso(&corpus, "uno", b"console.log(1);\n");
        let out = b.join("out");
        let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--minifier", "oxc", "--no-cache", "--jobs", "1"]);
        assert_eq!(rc(&o), Some(0), "{}", err(&o));
    }
    std::fs::remove_dir_all(&b).unwrap();
}

/// 1: error de entrada o de entorno. Ninguno necesita node.
#[test]
fn codigo_1_entrada_o_entorno() {
    let b = base("rc1");
    let vacio = b.join("vacio");
    std::fs::create_dir_all(&vacio).unwrap();
    let corpus = b.join("corpus");
    caso(&corpus, "uno", b"console.log(1);\n");
    let archivo = b.join("archivo");
    std::fs::write(&archivo, "x").unwrap();
    let out = b.join("out");
    let o = out.to_str().unwrap();
    let (no_existe, no_existe_js) = (b.join("no-existe"), b.join("no-existe.js"));
    let casos: [(&str, Vec<&str>); 5] = [
        ("corpus sin input.js", vec![vacio.to_str().unwrap(), "--out", o]),
        ("corpus inexistente", vec![no_existe.to_str().unwrap(), "--out", o]),
        ("--out es un archivo", vec![corpus.to_str().unwrap(), "--out", archivo.to_str().unwrap(), "--no-cache"]),
        ("--cruce sin indices", vec!["--cruce", vacio.to_str().unwrap()]),
        ("--emitir sin archivo", vec!["--emitir", no_existe_js.to_str().unwrap()]),
    ];
    for (que, args) in casos {
        let r = difuzz(&b, &args);
        assert_eq!(rc(&r), Some(1), "{que}: {}", err(&r));
        assert!(!err(&r).contains("uso: difuzz"), "{que}: un error de entorno no es de uso\n{}", err(&r));
    }
    std::fs::remove_dir_all(&b).unwrap();
}

/// 2: error de uso, con el USAGE. `--limit 0` es de uso como `--jobs 0` y ya
/// no borra la salida anterior de la celda (antes: 1, con `div/` borrado y
/// CORRIDA.txt en incompleta).
#[test]
fn codigo_2_uso() {
    let b = base("rc2");
    let corpus = b.join("corpus");
    caso(&corpus, "uno", b"console.log(1);\n");
    let out = b.join("out");
    let celda = out.join("swc+carril_a+tal_cual");
    std::fs::create_dir_all(celda.join("div/viejo")).unwrap();
    std::fs::write(celda.join("CORRIDA.txt"), "estado: completa\n").unwrap();
    let c = corpus.to_str().unwrap();
    let o = out.to_str().unwrap();
    for args in [
        vec![c, "--opcion-rara"],
        vec![c, "--jobs", "0"],
        vec![c, "--limit", "0", "--out", o],
        vec![c, "--limit", "x"],
        vec![],
        vec![c, "--cruce", o],
    ] {
        let r = difuzz(&b, &args);
        assert_eq!(rc(&r), Some(2), "{args:?}: {}", err(&r));
        assert!(err(&r).starts_with("error: ") && err(&r).contains("uso: difuzz"), "{args:?}: {}", err(&r));
    }
    assert!(celda.join("div/viejo").is_dir(), "--limit 0 borro la salida anterior");
    assert_eq!(std::fs::read_to_string(celda.join("CORRIDA.txt")).unwrap(), "estado: completa\n");
    std::fs::remove_dir_all(&b).unwrap();
}

/// Un campo `nombre=N` de la linea de stats.
fn campo(stats: &str, nombre: &str) -> usize {
    stats
        .split([' ', '('])
        .find_map(|t| t.strip_prefix(&format!("{nombre}=")))
        .and_then(|v| v.trim_end_matches(')').parse().ok())
        .unwrap_or_else(|| panic!("falta {nombre}= en {stats}"))
}

/// R2-H06: los mudos sin bindings y los ilegibles tienen su fila en
/// descartes.tsv, y `total=` se reconstruye caso por caso desde los archivos:
/// cada caso esta en un solo directorio de destino o en una sola fila, y el
/// resto son iguales.
#[test]
fn descartes_reconstruye_el_total() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("identidad");
    let corpus = b.join("corpus");
    caso(&corpus, "igual", b"console.log(1);\n");
    caso(&corpus, "mudo", b"// solo un comentario\n");
    caso(&corpus, "vacio", b"");
    caso(&corpus, "ilegible", &[0xff, 0xfe, 0x00, 0x41]);
    caso(&corpus, "sale", b"console.log(1);\nprocess.exit(3);\n");
    // known/ (fn_tostring) con oxc core_defaults, como en salida_celdas.
    caso(&corpus, "fts", b"function f(primero) { var segundo = primero + 1; return segundo; }\nconsole.log(f.toString());\n");
    let out = b.join("out");
    let o = difuzz(
        &b,
        &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--minifier", "oxc", "--config", "core_defaults", "--no-cache", "--jobs", "1"],
    );
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stats = stdout.lines().find(|l| l.starts_with("total=")).unwrap();
    let celda = out.join("oxc+core_defaults+tal_cual");
    let descartes = std::fs::read_to_string(celda.join("descartes.tsv")).unwrap();
    let filas: Vec<(&str, &str)> = descartes
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| {
            let mut c = l.split('\t');
            (c.next().unwrap(), c.next().unwrap())
        })
        .collect();
    for (c, causa) in [("mudo/input.js", "mudo_sin_bindings"), ("vacio/input.js", "mudo_sin_bindings"), ("ilegible/input.js", "ilegible"), ("sale/input.js", "exit")] {
        assert!(filas.contains(&(c, causa)), "falta {c} {causa} en\n{descartes}");
    }
    assert_eq!(campo(stats, "mudos_sin_bindings"), 2, "{stats}");
    assert_eq!(campo(stats, "ilegibles"), 1, "{stats}");
    // Caso por caso: los directorios de destino y las filas no se repiten.
    let mut casos: Vec<String> = filas.iter().map(|(c, _)| c.trim_end_matches("/input.js").to_string()).collect();
    for dest in ["div", "known", "panic", "recursos"] {
        if let Ok(rd) = std::fs::read_dir(celda.join(dest)) {
            casos.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()));
        }
    }
    let unicos: std::collections::BTreeSet<&String> = casos.iter().collect();
    assert_eq!(unicos.len(), casos.len(), "un caso en dos lugares: {casos:?}");
    assert!(celda.join("known/fts").is_dir(), "{stats}");
    let total = campo(stats, "total");
    assert_eq!(total, 6);
    let suma = campo(stats, "iguales") + campo(stats, "DIVERGENCIAS") + campo(stats, "conocidas") + campo(stats, "PANICS") + campo(stats, "RECURSOS") + filas.len();
    assert_eq!(total, suma, "{stats}\n{descartes}");
    assert_eq!(campo(stats, "iguales"), total - casos.len(), "{stats}");
    std::fs::remove_dir_all(&b).unwrap();
}
