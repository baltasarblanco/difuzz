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
            "--en-proceso", "--mem-hijo", "--cruce", "--emitir", "--crudo", "--help",
        ] {
            assert!(uso.contains(opt), "falta {opt} en la linea uso:\n{uso}");
        }
        assert!(s.contains("codigos de salida"), "{s}");
        assert!(s.contains("total = iguales + DIVERGENCIAS + conocidas + PANICS + RECURSOS + filas(descartes.tsv)"), "{s}");
    }
    // La ayuda como argumento propio, aunque haya un `-h` que es el valor de --out.
    let o = difuzz(&b, &["--out", "-h", "--help"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("uso: difuzz"));
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
    // E4: `--emitir -h` ya no es el archivo `-h` (1): es un error de uso (codigo_2_uso).
    let casos: [(&str, Vec<&str>); 6] = [
        ("corpus sin input.js", vec![vacio.to_str().unwrap(), "--out", o]),
        ("corpus inexistente", vec![no_existe.to_str().unwrap(), "--out", o]),
        ("--out es un archivo", vec![corpus.to_str().unwrap(), "--out", archivo.to_str().unwrap(), "--no-cache"]),
        ("--cruce sin indices", vec!["--cruce", vacio.to_str().unwrap()]),
        ("--emitir sin archivo", vec!["--emitir", no_existe_js.to_str().unwrap()]),
        // `./-h` como valor de --emitir es el archivo `-h` (que no existe), no la ayuda.
        ("--emitir ./-h", vec!["--emitir", "./-h"]),
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
    // La celda por defecto (B5: --minifier oxc), la que tocaria `--limit 0 --out`.
    let celda = out.join("oxc+carril_a+tal_cual");
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
        // `-h` como valor de una opcion no es la ayuda (antes: 0): --out sin corpus,
        // y un --minifier invalido.
        vec!["--out", "-h"],
        vec![c, "--minifier", "-h"],
        // E4: -h, --help u otra opcion como valor de una opcion de ruta (antes: la ruta).
        vec![c, "--out", "--help"],
        vec![c, "--cache", "-h"],
        vec![c, "--indice", "--jobs"],
        vec!["--emitir", "-h"],
        vec!["--cruce", "--help"],
    ] {
        let r = difuzz(&b, &args);
        assert_eq!(rc(&r), Some(2), "{args:?}: {}", err(&r));
        assert!(err(&r).starts_with("error: ") && err(&r).contains("uso: difuzz"), "{args:?}: {}", err(&r));
    }
    assert!(celda.join("div/viejo").is_dir(), "--limit 0 borro la salida anterior");
    assert_eq!(std::fs::read_to_string(celda.join("CORRIDA.txt")).unwrap(), "estado: completa\n");
    std::fs::remove_dir_all(&b).unwrap();
}

/// E4 (decisiones del 10-oct, fila 8): `corpus --out -h` y `corpus --out --help` salen con 2,
/// con el mensaje, y no crean nada (antes: el directorio `-h` o `--help` y 0). Corre en un
/// directorio propio, porque la ruta es relativa. `--out ./-h` sigue creando el directorio `-h`.
#[test]
fn codigo_2_opcion_como_ruta_no_crea_nada() {
    let b = base("ruta");
    let corpus = b.join("corpus");
    caso(&corpus, "uno", b"console.log(1);\n");
    let c = corpus.to_str().unwrap();
    let en_b = |args: &[&str]| {
        Command::new(BIN)
            .args(args)
            .current_dir(&b)
            .env("TMPDIR", b.join("tmp"))
            .env("DIFUZZ_ORACULO", "ninguno")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap()
    };
    for (opcion, valor, que) in [
        ("--out", "-h", "un directorio"),
        ("--out", "--help", "un directorio"),
        ("--cache", "-h", "un directorio"),
        ("--indice", "--help", "un directorio"),
    ] {
        let r = en_b(&[c, opcion, valor, "--jobs", "1"]);
        assert_eq!(rc(&r), Some(2), "{opcion} {valor}: {}", err(&r));
        let msg = format!("error: {opcion} espera una ruta y recibio la opcion {valor}; para {que} con ese nombre usa ./{valor}\n");
        assert!(err(&r).starts_with(&msg) && err(&r).contains("uso: difuzz"), "{opcion} {valor}: {}", err(&r));
        assert!(!b.join(valor).exists(), "{opcion} {valor}: creo {}", b.join(valor).display());
        assert!(r.stdout.is_empty(), "{opcion} {valor}: {}", String::from_utf8_lossy(&r.stdout));
    }
    let mut hay: Vec<String> = std::fs::read_dir(&b).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    hay.sort();
    assert_eq!(hay, ["corpus", "tmp"], "una corrida rechazada dejo algo");
    // `./-h` es la ruta: la corrida entera necesita node.
    if test_util::requiere_node() {
        let r = en_b(&[c, "--out", "./-h", "--minifier", "oxc", "--no-cache", "--jobs", "1"]);
        assert_eq!(rc(&r), Some(0), "{}", err(&r));
        assert!(b.join("-h/oxc+carril_a+tal_cual/CORRIDA.txt").is_file(), "{}", err(&r));
    }
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

/// 10.1: con `--crudo`, el crudo que da otra salida que el original impreso (los parentesis de
/// `(h) = function(){}`) va a codegen/, ortogonal: el caso cuenta igual donde termino, la linea
/// `total=` es la misma que sin la opcion y la diferencia solo de `toString` es `contenido`. Sin la
/// opcion no hay codegen/ ni linea `crudo`. El indice y `--cruce` cuentan el codegen/.
#[test]
fn crudo_va_a_codegen_y_sin_la_opcion_nada_cambia() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("crudo");
    let corpus = b.join("corpus");
    caso(&corpus, "paren", b"let h; (h) = function(){}; console.log(JSON.stringify(h.name));\n");
    caso(&corpus, "igual", b"console.log(1 + 1);\n");
    caso(&corpus, "fts", b"function f( a ) { return a }\nconsole.log(f.toString());\n");
    let correr = |out: &str, extra: &[&str]| {
        let (out, ix) = (b.join(out), b.join(format!("{out}-ix")));
        let mut args = vec![corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--indice", ix.to_str().unwrap(), "--no-cache", "--jobs", "1"];
        args.extend_from_slice(extra);
        let o = difuzz(&b, &args);
        assert_eq!(rc(&o), Some(0), "{}", err(&o));
        (String::from_utf8_lossy(&o.stdout).into_owned(), out.join("oxc+carril_a+tal_cual"), ix)
    };
    let (sin, celda_sin, _) = correr("sin", &[]);
    let (con, celda, ix) = correr("con", &["--crudo"]);
    let total = |s: &str| s.lines().find(|l| l.starts_with("total=")).unwrap().to_string();
    assert_eq!(total(&sin), total(&con));
    assert!(!sin.contains("crudo") && !celda_sin.join("codegen").exists(), "{sin}");
    let linea = con.lines().find(|l| l.starts_with("crudo (10.1): ")).unwrap_or_else(|| panic!("{con}"));
    for (c, n) in [("comparados", 3), ("identicos", 0), ("iguales", 1), ("CODEGEN", 1), ("contenido", 1), ("no_determinista", 0), ("timeout", 0), ("runner", 0)] {
        assert_eq!(campo(linea, c), n, "{c}: {linea}");
    }
    let meta = std::fs::read_to_string(celda.join("codegen/paren/meta.txt")).unwrap();
    assert!(meta.contains("kind: Stdout") && meta.contains("repro_crudo: "), "{meta}");
    assert_eq!(std::fs::read_to_string(celda.join("codegen/paren/crudo.stdout")).unwrap(), "\"\"\n");
    assert_eq!(std::fs::read_to_string(celda.join("codegen/paren/a.stdout")).unwrap(), "\"h\"\n");
    assert!(std::fs::read_to_string(celda.join("CORRIDA.txt")).unwrap().contains(linea));
    let cruce = difuzz(&b, &["--cruce", ix.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&cruce.stdout).contains("oxc+carril_a+tal_cual: div=0 known=1 panic=0 lint=0 recursos=0 codegen=1"), "{}", String::from_utf8_lossy(&cruce.stdout));
    std::fs::remove_dir_all(&b).unwrap();
}

/// 10.1b (H1 de las revisiones de 10.1): `contenido` solo si el paso `contenido` de A contra B
/// (`clasificar::paso_contenido`) atribuye la diferencia al texto de toString o de .stack. Los
/// bugs de los parentesis (O8 en oxc 0.153.0) en una rama que depende de ese texto (revision A) o
/// que solo se ven en un marco del stack (revision B), y uno ajeno a un toString que si importa,
/// van a codegen/ (antes, los tres primeros eran `contenido`). Siguen en `contenido` un toString
/// impreso entero y una diferencia que solo es la columna de un marco.
#[test]
fn crudo_contenido_solo_si_el_contenido_explica() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("crudo-contenido");
    let corpus = b.join("corpus");
    let bugs: [(&str, &[u8]); 4] = [
        ("rama-tostring", b"function g() { return 1; }\nlet h;\nif (String(g).includes(\"return\")) { (h) = function () {}; }\nconsole.log(JSON.stringify(h ? h.name : null));\n"),
        ("rama-stack", b"let h;\nif (new Error(\"x\").stack.includes(\"(\")) { (h) = function () {}; }\nconsole.log(JSON.stringify(h ? h.name : null));\n"),
        ("marco", b"let h; (h) = function(){ return new Error(\"x\").stack.split(\"\\n\")[1].includes(\" h \") }; console.log(h());\n"),
        ("ajeno", b"function g() { return 1; }\nlet h; (h) = function () {};\nconsole.log(String(g).length, JSON.stringify(h.name));\n"),
    ];
    for (n, src) in bugs {
        caso(&corpus, n, src);
    }
    caso(&corpus, "fts", b"function f( a ) { return a }\nconsole.log(f.toString());\n");
    caso(&corpus, "columna", b"var               s = new Error(\"x\").stack;\nconsole.log(/:1:\\d\\d\\)/.test(s), s.split(\"\\n\").length);\n");
    let (out, ix) = (b.join("out"), b.join("ix"));
    let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--indice", ix.to_str().unwrap(), "--no-cache", "--jobs", "2", "--crudo"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    let linea = stdout.lines().find(|l| l.starts_with("crudo (10.1): ")).unwrap_or_else(|| panic!("{stdout}"));
    for (c, n) in [("comparados", 6), ("CODEGEN", 4), ("contenido", 2), ("no_determinista", 0), ("timeout", 0), ("runner", 0)] {
        assert_eq!(campo(linea, c), n, "{c}: {linea}\n{}", err(&o));
    }
    let codegen = out.join("oxc+carril_a+tal_cual/codegen");
    for (n, _) in bugs {
        assert!(codegen.join(n).join("meta.txt").is_file(), "falta codegen/{n}: {}", err(&o));
    }
    let meta = std::fs::read_to_string(codegen.join("ajeno/meta.txt")).unwrap();
    assert!(meta.contains("contenido: con el contenido fijo"), "{meta}");
    let meta = std::fs::read_to_string(codegen.join("rama-tostring/meta.txt")).unwrap();
    assert!(meta.contains("(salado) da lo mismo"), "{meta}");
    std::fs::remove_dir_all(&b).unwrap();
}

/// 10.1b (H2 de la revision A de 10.1): un timeout del crudo es un timeout. Si se cuelga uno solo
/// (el crudo imprimiendo o mudo, o A), confirmado con el timeout largo, va a codegen/ con kind
/// Timeout (antes: `no_determinista` si imprimia, kind Stdout si no); si se cuelgan los dos,
/// `timeout=`. Un crudo que no se repite a si mismo (el pid) es `no_determinista` (la doble
/// repeticion; revision A, H7).
#[test]
fn crudo_timeout_es_timeout() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("crudo-timeout");
    let corpus = b.join("corpus");
    let colgados: [(&str, &[u8]); 3] = [
        ("imprime", b"let h;\n(h) = function () {};\nlet i = 0;\nwhile (h.name === \"\") { if (++i % 100000 === 0) console.log(i); }\nconsole.log(\"fin\");\n"),
        ("mudo", b"let h;\n(h) = function () {};\nwhile (h.name === \"\") {}\nconsole.log(\"fin\");\n"),
        ("a-colgado", b"let h;\n(h) = function () {};\nwhile (h.name === \"h\") {}\nconsole.log(\"fin\");\n"),
    ];
    for (n, src) in colgados {
        caso(&corpus, n, src);
    }
    caso(&corpus, "los-dos", b"let i = 0;\nwhile (true) { if (++i % 100000 === 0) console.log(i); }\n");
    caso(&corpus, "pid", b"console.log(require(\"fs\").readFileSync(\"/proc/self/stat\", \"utf8\").split(\" \")[0]);\n");
    let out = b.join("out");
    let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "5", "--timeout-ms", "1000", "--crudo"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    let linea = stdout.lines().find(|l| l.starts_with("crudo (10.1): ")).unwrap_or_else(|| panic!("{stdout}"));
    for (c, n) in [("comparados", 5), ("iguales", 0), ("CODEGEN", 3), ("contenido", 0), ("no_determinista", 1), ("timeout", 1), ("runner", 0)] {
        assert_eq!(campo(linea, c), n, "{c}: {linea}\n{}", err(&o));
    }
    for (n, _) in colgados {
        let meta = std::fs::read_to_string(out.join("oxc+carril_a+tal_cual/codegen").join(n).join("meta.txt")).unwrap_or_else(|e| panic!("{n}: {e}\n{}", err(&o)));
        assert!(meta.contains("kind: Timeout") && meta.contains("en Timeout: repetido con"), "{n}: {meta}");
    }
    assert!(err(&o).contains("crudo: no determinista (el crudo o A no se repiten): pid"), "{}", err(&o));
    std::fs::remove_dir_all(&b).unwrap();
}

/// 9.4: `difuzz` con stdout y/o stderr en un pipe cerrado (el que lee ya se fue: EPIPE). Antes,
/// rc 101 (panic de `println!`/`eprintln!`) y, con stderr cerrado a mitad de corrida, la
/// divergencia se perdia: el panic del `DIVERGENCIA ...` llegaba antes que el reporter.
/// Devuelve el rc y lo que salio por el stream que sigue abierto.
fn difuzz_con_pipe_cerrado(base: &Path, args: &[&str], stdout: bool, stderr: bool) -> (Option<i32>, String) {
    let mut c = Command::new(BIN);
    c.args(args).env("TMPDIR", base.join("tmp")).env("DIFUZZ_ORACULO", "ninguno");
    let (lectura, escritura) = std::io::pipe().unwrap();
    drop(lectura);
    c.stdout(if stdout { Stdio::from(escritura.try_clone().unwrap()) } else { Stdio::piped() });
    c.stderr(if stderr { Stdio::from(escritura) } else { Stdio::piped() });
    let o = c.output().unwrap();
    let mut leido = String::new();
    for (cerrado, bytes) in [(stdout, &o.stdout), (stderr, &o.stderr)] {
        if !cerrado {
            leido.push_str(&String::from_utf8_lossy(bytes));
        }
    }
    (o.status.code(), leido)
}

#[test]
fn pipe_cerrado_ayuda_y_uso() {
    let b = base("pipe-uso");
    for (stdout, stderr) in [(true, false), (false, true), (true, true)] {
        let (rc, leido) = difuzz_con_pipe_cerrado(&b, &["--help"], stdout, stderr);
        assert_eq!(rc, Some(0), "--help, stdout cerrado={stdout} stderr cerrado={stderr}: {leido}");
        let (rc, leido) = difuzz_con_pipe_cerrado(&b, &["--opcion-rara"], stdout, stderr);
        assert_eq!(rc, Some(2), "uso, stdout cerrado={stdout} stderr cerrado={stderr}: {leido}");
    }
    // Un stdout que no se puede escribir por otra cosa (disco lleno) si es un error: 1.
    if Path::new("/dev/full").exists() {
        let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
        let o = Command::new(BIN).arg("--help").stdout(Stdio::from(full)).stderr(Stdio::piped()).output().unwrap();
        assert_eq!(rc(&o), Some(1), "{}", err(&o));
        assert!(err(&o).starts_with("no pude escribir en stdout: "), "{}", err(&o));
    }
    std::fs::remove_dir_all(&b).unwrap();
}

#[test]
fn pipe_cerrado_no_pierde_la_divergencia() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("pipe-div");
    let corpus = b.join("corpus");
    // golden oxc-o1: div:ThrowNew en oxc carril_a (tests/golden/esperado.tsv).
    let o1 = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/corpus/oxc-o1/input.js");
    caso(&corpus, "o1", &std::fs::read(o1).unwrap());
    caso(&corpus, "igual", b"console.log(1);\n");
    for (stdout, stderr) in [(false, true), (true, false), (true, true)] {
        let out = b.join(format!("out-{stdout}-{stderr}"));
        let args = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--minifier", "oxc", "--no-cache", "--jobs", "1"];
        let (rc, leido) = difuzz_con_pipe_cerrado(&b, &args, stdout, stderr);
        let que = format!("stdout cerrado={stdout} stderr cerrado={stderr}");
        assert_eq!(rc, Some(0), "{que}: {leido}");
        let celda = out.join("oxc+carril_a+tal_cual");
        assert!(celda.join("div/o1").is_dir(), "{que}: falta div/o1\n{leido}");
        let corrida = std::fs::read_to_string(celda.join("CORRIDA.txt")).unwrap();
        assert!(corrida.starts_with("estado: completa\n"), "{que}: {corrida}");
        if !stdout {
            let stats = leido.lines().find(|l| l.starts_with("total=")).unwrap_or_else(|| panic!("{que}: {leido}"));
            assert_eq!((campo(stats, "total"), campo(stats, "DIVERGENCIAS"), campo(stats, "panics_del_fuzzer")), (2, 1, 0), "{stats}");
        }
    }
    std::fs::remove_dir_all(&b).unwrap();
}
