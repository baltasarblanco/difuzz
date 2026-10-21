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
    // 11.2: antes, `process.exit(3)` (descarte `exit`); ahora ese original se compara.
    caso(&corpus, "enlace", b"require(\"./no-existe-difuzz-salida\");\n");
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
    for (c, causa) in [("mudo/input.js", "mudo_sin_bindings"), ("vacio/input.js", "mudo_sin_bindings"), ("ilegible/input.js", "ilegible"), ("enlace/input.js", "enlace")] {
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
/// `(h) = function(){}`, O8, oxc#6154) va a codegen/, ortogonal: el caso cuenta igual donde termino, la linea
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
    let linea = con.lines().find(|l| l.starts_with("crudo: ")).unwrap_or_else(|| panic!("{con}"));
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
/// bugs de los parentesis (O8 en oxc 0.153.0, oxc#6154) en una rama que depende de ese texto (revision A) o
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
    let linea = stdout.lines().find(|l| l.starts_with("crudo: ")).unwrap_or_else(|| panic!("{stdout}"));
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
/// Timeout (antes: `no_determinista` si imprimia, kind Stdout si no); si se cuelgan los dos sin
/// imprimir, `timeout=`. Un crudo que no se repite a si mismo (el pid) es `no_determinista` (la
/// doble repeticion; revision A, H7).
/// 10.1c: si se cuelgan los dos despues de imprimir otra cosa (`""` contra `"h"`), codegen/ con kind
/// Stdout (M3 de la revision B de 10.1b; antes `timeout=` sin directorio). Un A fresco que no
/// repite al A del caso (un archivo marca) es `no_determinista` (N1 de la revision A de 10.1b;
/// antes `iguales`). Timeout de 1500 ms: el arranque de node con el preload
/// entra con margen aun con la maquina cargada (revision A de 10.1b, N5), y un A que termino en el
/// caso y se cuelga en las frescas por la carga se repite con el timeout largo.
#[test]
fn crudo_timeout_es_timeout() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("crudo-timeout");
    let corpus = b.join("corpus");
    let colgados: [(&str, &[u8], &str); 4] = [
        ("imprime", b"let h;\n(h) = function () {};\nlet i = 0;\nwhile (h.name === \"\") { if (++i % 100000 === 0) console.log(i); }\nconsole.log(\"fin\");\n", "kind: Timeout"),
        ("mudo", b"let h;\n(h) = function () {};\nwhile (h.name === \"\") {}\nconsole.log(\"fin\");\n", "kind: Timeout"),
        ("a-colgado", b"let h;\n(h) = function () {};\nwhile (h.name === \"h\") {}\nconsole.log(\"fin\");\n", "kind: Timeout"),
        ("los-dos-distinto", b"let h; (h) = function(){}; console.log(JSON.stringify(h.name)); while (true) {}\n", "kind: Stdout"),
    ];
    for (n, src, _) in colgados {
        caso(&corpus, n, src);
    }
    caso(&corpus, "los-dos", b"while (true) {}\n");
    caso(&corpus, "pid", b"console.log(require(\"fs\").readFileSync(\"/proc/self/stat\", \"utf8\").split(\" \")[0]);\n");
    // La primera corrida (el A del caso) crea la marca e imprime `"h"`; las siguientes (el crudo
    // y todas las frescas) imprimen `otra`. Antes, los frescos coincidian entre si y daba `iguales`.
    // El repro de la revision (las siguientes se cuelgan) se midio aparte: con las corridas de A
    // contra B colgadas, cuesta mas de un minuto.
    let marca = b.join("marca.txt");
    let src = format!(
        "const fs = require(\"fs\");\nconst p = {:?};\nlet h;\n(h) = function () {{}};\nif (fs.existsSync(p)) {{ console.log(\"otra\"); }} else {{ fs.writeFileSync(p, \"x\"); console.log(JSON.stringify(h.name)); }}\n",
        marca.to_str().unwrap()
    );
    caso(&corpus, "marca", src.as_bytes());
    let out = b.join("out");
    let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "7", "--timeout-ms", "1500", "--crudo"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    let linea = stdout.lines().find(|l| l.starts_with("crudo: ")).unwrap_or_else(|| panic!("{stdout}"));
    for (c, n) in [("comparados", 7), ("iguales", 0), ("CODEGEN", 4), ("contenido", 0), ("no_determinista", 2), ("timeout", 1), ("runner", 0)] {
        assert_eq!(campo(linea, c), n, "{c}: {linea}\n{}", err(&o));
    }
    for (n, _, kind) in colgados {
        let meta = std::fs::read_to_string(out.join("oxc+carril_a+tal_cual/codegen").join(n).join("meta.txt")).unwrap_or_else(|e| panic!("{n}: {e}\n{}", err(&o)));
        assert!(meta.contains(kind), "{n}: {meta}");
        if kind == "kind: Timeout" {
            assert!(meta.contains("en Timeout: repetido con"), "{n}: {meta}");
        }
    }
    for n in ["pid", "marca"] {
        assert!(err(&o).contains(&format!("crudo: no determinista (el crudo o A no se repiten): {n}/")), "{n}: {}", err(&o));
    }
    // 10.1d (R1 de la revision A de 10.1c): los dos colgados dejan rastro por stderr.
    assert!(err(&o).contains("crudo: los dos se cuelgan: los-dos/"), "{}", err(&o));
    std::fs::remove_dir_all(&b).unwrap();
}

/// 10.1d (N1 de la revision B de 10.1c): con `--cache`, un A del caso que sale de una entrada vieja
/// (el programa lee un archivo que cambio entre corridas) y no repite a los A frescos, que si se
/// repiten entre si, no hace `no_determinista` una diferencia confirmable: la entrada de A se lapida
/// (como en `confirm`) y el crudo se compara con el A fresco. Antes de 10.1c seguia con el fresco;
/// en 10.1c daba `no_determinista`.
#[test]
fn crudo_con_cache_vieja_de_a_sigue_con_el_fresco() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("crudo-cache-vieja");
    let corpus = b.join("corpus");
    let flag = b.join("flag.txt");
    std::fs::write(&flag, "X").unwrap();
    let src = format!(
        "let h; (h) = function(){{}}; console.log(require(\"fs\").readFileSync({:?}, \"utf8\"), JSON.stringify(h.name));\n",
        flag.to_str().unwrap()
    );
    caso(&corpus, "c1", src.as_bytes());
    let cache = b.join("cache");
    let correr = |out: &str| {
        let out = b.join(out);
        let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--cache", cache.to_str().unwrap(), "--jobs", "1", "--crudo"]);
        assert_eq!(rc(&o), Some(0), "{}", err(&o));
        let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
        let linea = stdout.lines().find(|l| l.starts_with("crudo: ")).unwrap_or_else(|| panic!("{stdout}")).to_string();
        (linea, err(&o), out.join("oxc+carril_a+tal_cual/codegen/c1/meta.txt"))
    };
    let (linea, e, _) = correr("out1");
    assert_eq!((campo(&linea, "CODEGEN"), campo(&linea, "no_determinista")), (1, 0), "{linea}\n{e}");
    std::fs::write(&flag, "Y").unwrap();
    let (linea, e, meta) = correr("out2");
    assert_eq!((campo(&linea, "CODEGEN"), campo(&linea, "no_determinista"), campo(&linea, "cache_hits")), (1, 0, 1), "{linea}\n{e}");
    let meta = std::fs::read_to_string(meta).unwrap();
    assert!(meta.contains("se lapida la entrada de A y sigue el A fresco"), "{meta}");
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

/// 10.5 (M2): `--preparar --info` (la identidad del oraculo) sin `println!`. Antes,
/// rc 101 (panic) con el stdout en un pipe cerrado o en /dev/full.
#[test]
fn preparar_info_sin_panic() {
    let b = base("pipe-info");
    let (codigo, leido) = difuzz_con_pipe_cerrado(&b, &["--preparar", "--info"], true, false);
    assert_eq!(codigo, Some(0), "pipe cerrado: {leido}");
    assert!(!leido.contains("panicked"), "{leido}");
    let o = difuzz(&b, &["--preparar", "--info"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    assert!(!o.stdout.is_empty() && o.stdout.ends_with(b"\n"), "{:?}", String::from_utf8_lossy(&o.stdout));
    if Path::new("/dev/full").exists() {
        let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
        let o = Command::new(BIN).args(["--preparar", "--info"]).stdout(Stdio::from(full)).stderr(Stdio::piped()).output().unwrap();
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
    // golden oxc-o1 (el Issue C, oxc#27863): div:ThrowNew en oxc carril_a (tests/golden/esperado.tsv).
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

/// 10.2b (B-H1 y B-H2 de la revision B del bloque 10): una corrida con `--indice` que pierde
/// escrituras no publica un indice como completo. (a) El indice no se puede crear (el `.tmp` de la
/// celda es un directorio): antes rc 0, `completa` y `--cruce` mostraba la corrida anterior sin
/// aviso. (b) `known-sintomas.tsv` no se puede guardar (es un directorio): antes rc 0 y `completa`
/// y el indice se publicaba. Ahora rc 1, `incompleta`, el indice de la corrida queda `.parcial` y
/// `--cruce` avisa.
#[test]
fn indice_con_escrituras_perdidas_no_se_publica() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("indice-perdido");
    let corpus = b.join("corpus");
    let o1 = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/corpus/oxc-o1/input.js");
    caso(&corpus, "o1", &std::fs::read(o1).unwrap());
    let (ix, celda) = (b.join("ix"), "oxc+carril_a+tal_cual");
    let correr = |out: &str| {
        let out = b.join(out);
        let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--indice", ix.to_str().unwrap(), "--no-cache", "--jobs", "1"]);
        let corrida = std::fs::read_to_string(out.join(celda).join("CORRIDA.txt")).unwrap_or_default();
        (rc(&o), err(&o), corrida)
    };
    let indice = |suf: &str| ix.join(format!("index-{celda}.tsv{suf}"));
    // Una corrida completa: el indice vigente.
    let (r, e, corrida) = correr("out0");
    assert_eq!(r, Some(0), "{e}");
    assert!(corrida.starts_with("estado: completa\n") && indice("").is_file(), "{corrida}");
    let cruce = |b: &Path| String::from_utf8_lossy(&difuzz(b, &["--cruce", ix.to_str().unwrap()]).stdout).into_owned();
    assert!(!cruce(&b).contains("incompleta"));
    // (a) el indice no se puede crear.
    std::fs::create_dir_all(indice(".tmp").join("x")).unwrap();
    let (r, e, corrida) = correr("out1");
    assert_eq!(r, Some(1), "{e}");
    assert!(e.contains("no pude crear el indice"), "{e}");
    // 10.2c (N2): el mensaje final no le atribuye al reporter en <out> el indice que no se creo.
    assert!(!e.contains("el reporter no pudo escribir") && e.contains("una es el indice, que no se pudo crear (--indice)"), "{e}");
    assert!(corrida.starts_with("estado: incompleta\n") && corrida.contains("\nindice_no_creado: "), "{corrida}");
    assert!(indice(".parcial").is_file() && indice("").is_file());
    let t = cruce(&b);
    assert!(t.contains(&format!("\n{celda}: incompleta (escrituras perdidas, casos sin contar o sin indice)\n")), "{t}");
    std::fs::remove_dir_all(indice(".tmp")).unwrap();
    // Una corrida completa borra la marca.
    let (r, e, _) = correr("out2");
    assert_eq!(r, Some(0), "{e}");
    assert!(!indice(".parcial").exists() && !cruce(&b).contains("incompleta"));
    // (b) known-sintomas.tsv no se puede guardar: el indice de la corrida no reemplaza al vigente.
    let vigente = std::fs::read_to_string(indice("")).unwrap();
    std::fs::create_dir_all(b.join("out3").join(celda).join("known-sintomas.tsv/x")).unwrap();
    let (r, e, corrida) = correr("out3");
    assert_eq!(r, Some(1), "{e}");
    assert!(e.contains("no pude guardar known-sintomas.tsv"), "{e}");
    assert!(corrida.starts_with("estado: incompleta\n") && corrida.contains("\nescrituras_perdidas: 1\n"), "{corrida}");
    assert!(indice(".parcial").is_file() && !indice(".tmp").exists());
    assert_eq!(std::fs::read_to_string(indice("")).unwrap(), vigente);
    assert!(cruce(&b).contains("incompleta (escrituras perdidas"));
    std::fs::remove_dir_all(&b).unwrap();
}

/// 11.5 (I3): una ejecucion del original (A) que pasa `--mem-node` manda el caso a recursos/ (causa
/// `node_memoria`, con el programa que corrio); nunca a iguales. 11.2b (H1 y
/// H2): el que solo reserva de mas minificado (`solo-b`: mide su propio texto) no va a recursos/: B
/// se repite con los topes mayores, lo vuelve a pasar y es una divergencia (ExitStatus, con el paso
/// `tope_node` en meta.txt). Un re-chequeo que pasa el tope (`d01`: el de profundidad reserva el doble)
/// queda Inconcluso y el caso sigue en div/. Una variante de `--mutaciones` que pasa el tope va a
/// recursos/ ella sola (con su `variante.js`) y su base sigue en iguales. La identidad de `total=` cierra.
#[test]
fn tope_de_memoria_de_node_va_a_recursos() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("tope-node");
    let corpus = b.join("corpus");
    let reservar = "const b = [];\nfor (let i = 0; i < 64; i++) b.push(Buffer.alloc(32 << 20, 1));";
    caso(&corpus, "igual", b"console.log(1);\n");
    caso(&corpus, "mem", format!("{reservar}\nconsole.log(b.length);\n").as_bytes());
    let relleno = "x".repeat(300);
    caso(
        &corpus,
        "solo-b",
        format!("(function () {{ const relleno = \"{relleno}\"; }})();\nif (require(\"fs\").readFileSync(__filename, \"utf8\").length < 300) {{\n{reservar}\n}}\nconsole.log(\"listo\");\n").as_bytes(),
    );
    let out = b.join("out");
    let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--mem-node", "256", "--no-cache", "--jobs", "1"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stats = stdout.lines().find(|l| l.starts_with("total=")).unwrap();
    for (c, n) in [("total", 3), ("iguales", 1), ("DIVERGENCIAS", 1), ("RECURSOS", 1), ("node_memoria", 1), ("panics_del_fuzzer", 0)] {
        assert_eq!(campo(stats, c), n, "{c}: {stats}");
    }
    assert!(err(&o).contains("topes de node: 256 MiB de RSS por ejecucion (--mem-node) y 16 MiB de salida"), "{}", err(&o));
    let celda = out.join("oxc+carril_a+tal_cual");
    let meta = std::fs::read_to_string(celda.join("recursos/mem/meta.txt")).unwrap();
    assert!(meta.contains("causa: node_memoria\n") && meta.contains("paso el tope de RSS de node") && meta.contains("> 256 MiB"), "{meta}");
    // 11.2b (A-H2/B-H3): que ejecucion fue y con que argumentos.
    assert!(meta.contains("ejecucion: el original (A) del caso") && meta.contains("corrio como: ") && meta.contains(" --require preload.cjs programa.cjs (timeout 5000 ms;"), "{meta}");
    assert!(std::fs::read_to_string(celda.join("recursos/mem/programa.cjs")).unwrap().contains("Buffer.alloc"));
    let meta = std::fs::read_to_string(celda.join("div/solo-b/meta.txt")).unwrap();
    assert!(meta.contains("kind: ExitStatus\n") && meta.contains("tope_node: B (el minificado) paso el tope de node (node_memoria)"), "{meta}");
    assert!(meta.contains("repetido con topes mayores (512 MiB de RSS, 64 MiB de salida, sin el sensor, timeout ") && meta.contains(" ms) lo vuelve a pasar"), "{meta}");
    let filas = std::fs::read_to_string(celda.join("descartes.tsv")).unwrap().lines().filter(|l| !l.starts_with('#')).count();
    assert_eq!(campo(stats, "total"), campo(stats, "iguales") + campo(stats, "DIVERGENCIAS") + campo(stats, "RECURSOS") + filas, "{stats}");

    // Un re-chequeo que pasa el tope: 24-f1-d01 de la aceptacion (bug F de oxc, oxc#27098, div:Stdout) con una
    // reserva proporcional a la profundidad de pila: A y B ~240 MiB,
    // la corrida honda del re-chequeo de profundidad el doble.
    let corpus_r = b.join("corpus-r");
    let d01 = "let d = 0; function r() { d++; r(); } try { r(); } catch {}\nconst reserva = Buffer.alloc(d * 16000, 1);\nlet a = [];\nfor (let i = 0; i < 7000; i++) a = [a];\nfunction h(v) {\n  const t = [\"\", \"abc\".substring(1, undefined)];\n  console.log(JSON.stringify(t[+(typeof v !== \"string\")]));\n}\nPromise.resolve(a).then(JSON.stringify).then(h, h);\n";
    caso(&corpus_r, "d01", d01.as_bytes());
    let out_r = b.join("out-r");
    let o = difuzz(&b, &[corpus_r.to_str().unwrap(), "--out", out_r.to_str().unwrap(), "--mem-node", "320", "--no-cache", "--jobs", "1"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stats = stdout.lines().find(|l| l.starts_with("total=")).unwrap();
    assert_eq!((campo(stats, "DIVERGENCIAS"), campo(stats, "RECURSOS")), (1, 0), "{stats}");
    let meta = std::fs::read_to_string(out_r.join("oxc+carril_a+tal_cual/div/d01/meta.txt")).unwrap();
    assert!(meta.contains("kind: Stdout\n") && meta.contains("profundidad: Inconcluso -> profundidad_inconcluso: una ejecucion del re-chequeo paso un tope de node"), "{meta}");
    // 11.2b (A-H2): la del re-chequeo de profundidad, con --stack-size.
    assert!(meta.contains("--stack-size=1968"), "{meta}");

    // Una variante: `n` perturbado reserva (el base no). Las variantes son deterministas (semilla
    // del texto); con 30 + 10 alguna toca `n`.
    let corpus_v = b.join("corpus-v");
    caso(&corpus_v, "v1", format!("const n = 0;\nif (n) {{\n{reservar}\n}}\nconsole.log(\"listo\", n);\n").as_bytes());
    let out_v = b.join("out-v");
    let args = [corpus_v.to_str().unwrap(), "--out", out_v.to_str().unwrap(), "--mem-node", "256", "--mutaciones", "30", "--mutaciones-extra", "10", "--no-cache", "--jobs", "1"];
    let o = difuzz(&b, &args);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stats = stdout.lines().find(|l| l.starts_with("total=")).unwrap();
    assert_eq!((campo(stats, "iguales"), campo(stats, "RECURSOS")), (1, 0), "{stats}");
    let mutaciones = stdout.lines().find(|l| l.starts_with("mutaciones:")).unwrap();
    let rec = campo(mutaciones, "RECURSOS");
    assert!(rec >= 1, "{mutaciones}");
    let dirs: Vec<std::path::PathBuf> = std::fs::read_dir(out_v.join("oxc+carril_a+tal_cual+mut30+mutx10/recursos")).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(dirs.len(), rec, "{dirs:?}");
    for d in dirs {
        assert!(d.file_name().unwrap().to_string_lossy().starts_with("v1__mut"), "{d:?}");
        let meta = std::fs::read_to_string(d.join("meta.txt")).unwrap();
        assert!(meta.contains("causa: node_memoria\n"), "{meta}");
        assert!(d.join("variante.js").is_file() && d.join("programa.cjs").is_file(), "{d:?}");
    }
    std::fs::remove_dir_all(&b).unwrap();
}

/// 11.7 (I5): un original que escribe mas que `--max-salida` (stdout mas stderr) manda el caso a
/// recursos/ (causa `node_salida`), nunca a iguales: lo juntado esta cortado. Por stdout (`grande`) o
/// por stderr (`err`). 11.2b: el que solo escribe de mas minificado (`solo-b`, 3 MiB
/// contra 1 MiB de A) no va a recursos/: B se repite con el tope mayor (8 MiB), termina y diverge por
/// stdout, como sin topes. Lo que entra en el tope se compara como siempre (`entra`).
#[test]
fn tope_de_salida_de_node_va_a_recursos() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("tope-salida");
    let corpus = b.join("corpus");
    let escribir = |n: usize, por: &str| format!("const s = \"x\".repeat(1 << 20) + \"\\n\";\nfor (let i = 0; i < {n}; i++) process.{por}.write(s);\n");
    caso(&corpus, "entra", format!("{}console.log(\"fin\");\n", escribir(1, "stdout")).as_bytes());
    caso(&corpus, "grande", escribir(3, "stdout").as_bytes());
    caso(&corpus, "err", format!("console.log(1);\n{}", escribir(3, "stderr")).as_bytes());
    let relleno = "x".repeat(300);
    caso(
        &corpus,
        "solo-b",
        format!(
            "(function () {{ const relleno = \"{relleno}\"; }})();\nconst n = require(\"fs\").readFileSync(__filename, \"utf8\").length < 300 ? 3 : 1;\n{}",
            escribir(1, "stdout").replace("i < 1", "i < n")
        )
        .as_bytes(),
    );
    let out = b.join("out");
    let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--max-salida", "2", "--no-cache", "--jobs", "1"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stats = stdout.lines().find(|l| l.starts_with("total=")).unwrap();
    for (c, n) in [("total", 4), ("iguales", 1), ("DIVERGENCIAS", 1), ("RECURSOS", 2), ("node_salida", 2), ("node_memoria", 0), ("panics_del_fuzzer", 0)] {
        assert_eq!(campo(stats, c), n, "{c}: {stats}");
    }
    assert!(err(&o).contains(" y 2 MiB de salida (--max-salida)"), "{}", err(&o));
    let celda = out.join("oxc+carril_a+tal_cual");
    for c in ["grande", "err"] {
        let meta = std::fs::read_to_string(celda.join("recursos").join(c).join("meta.txt")).unwrap();
        assert!(meta.contains("causa: node_salida\n") && meta.contains("paso el tope de salida de 2 MiB"), "{meta}");
        assert!(celda.join("recursos").join(c).join("programa.cjs").is_file());
    }
    let meta = std::fs::read_to_string(celda.join("div/solo-b/meta.txt")).unwrap();
    assert!(meta.contains("kind: Stdout\n") && meta.contains("repetido con topes mayores") && meta.contains("8 MiB de salida, sin el sensor, timeout ") && meta.contains(" ms) termino y se compara con eso"), "{meta}");
    let filas = std::fs::read_to_string(celda.join("descartes.tsv")).unwrap().lines().filter(|l| !l.starts_with('#')).count();
    assert_eq!(campo(stats, "total"), campo(stats, "iguales") + campo(stats, "DIVERGENCIAS") + campo(stats, "RECURSOS") + filas, "{stats}");
    std::fs::remove_dir_all(&b).unwrap();
}

/// Los textos de un caso que solo cambia en el minificado (B, mas corto: el relleno muerto se va).
fn solo_b(cuerpo_b: &str, cuerpo_a: &str, cola: &str) -> String {
    let relleno = "x".repeat(2000);
    format!("(function () {{ const relleno = \"{relleno}\"; }})();\nconst corto = require(\"fs\").readFileSync(__filename, \"utf8\").length < 2000;\nif (corto) {{\n{cuerpo_b}\n}} else {{\n{cuerpo_a}\n}}\n{cola}\n")
}

/// 11.2c: las ramas del Timeout de B en `confirm`.
/// - `bucle-lento` (el t03 de la revision A de 11b, lento): B, con el bug F de oxc (oxc#27098), imprime sin fin
///   64 KiB cada 100 ms. Con el timeout de 1 s no llega al tope de salida (1 MiB); con el timeout
///   largo lo pasa: queda el Timeout, `div/` (con MUT-a, iguales: un miscompile real perdido).
/// - `mayor-gira`: B escribe 1,5 MiB (pasa el tope) y gira sin fin: repetido con los topes mayores
///   (4 MiB) y el timeout largo no termina: `div/` ExitStatus con el B cortado (con MUT-b, iguales).
/// - `c02` (control) y `c03` de la revision A: B sin bug, mas lento que el timeout; c03 ademas
///   reserva 400 MiB (entre el tope de 300 y el mayor de 600). Los dos, iguales: un B mas lento y
///   mas pesado que A no es un hallazgo (antes c03 iba a `div/` ExitStatus: la repeticion con los
///   topes mayores corria con el timeout normal).
#[test]
fn confirmacion_de_b_con_timeout_largo() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("b-timeout-largo");
    let corpus = b.join("corpus");
    let f = "const n = \"abc\".substring(1, undefined).length;";
    // `fs.writeSync` con reintento: `console.log` a un pipe no bloqueante encola en node mientras
    // `Atomics.wait` frena el bucle de eventos, y la salida no llegaba nunca al tope.
    caso(
        &corpus,
        "bucle-lento",
        format!("{f}\nconst ia = new Int32Array(new SharedArrayBuffer(4));\nif (n === 0) {{\n  const fs = require(\"fs\");\n  const s = Buffer.alloc(64 * 1024, 120);\n  for (;;) {{\n    let k = 0;\n    while (k < s.length) {{ try {{ k += fs.writeSync(1, s, k); }} catch (e) {{}} }}\n    Atomics.wait(ia, 0, 0, 100);\n  }}\n}}\nconsole.log(\"fin\", n);\n").as_bytes(),
    );
    caso(
        &corpus,
        "mayor-gira",
        format!("{f}\nif (n === 0) {{\n  const fs = require(\"fs\");\n  const s = Buffer.alloc(1 << 16, 121);\n  let total = 0;\n  while (total < 1536 * 1024) {{ try {{ total += fs.writeSync(1, s); }} catch (e) {{}} }}\n  for (;;) {{}}\n}}\nconsole.log(\"fin\", n);\n").as_bytes(),
    );
    let esperar = |ms: u32| format!("Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, {ms});");
    caso(&corpus, "c02", solo_b(&esperar(1300), &esperar(300), "console.log(\"listo\");").as_bytes());
    caso(
        &corpus,
        "c03",
        solo_b(&format!("const keep = [];\nfor (let i = 0; i < 8; i++) keep.push(Buffer.alloc(50 << 20, 1));\n{}", esperar(1300)), &esperar(300), "console.log(\"listo\");").as_bytes(),
    );
    // 11.2e: s01, sin bug. Solo B reserva 300 MiB (pasa el tope de
    // 300, no el mayor) y atrapa 300 excepciones a 1000 marcos de profundidad: con el sensor tarda
    // segundos (MEDIDO por la revision: 6,5 s), sin el, 0,2 s. La repeticion con los topes mayores corre
    // sin el sensor: iguales (con 705d3c5, con el sensor y sin reintento, div/ ExitStatus).
    let lento_con_sensor = "function r(n) {\n  if (n === 0) {\n    let c = 0;\n    for (let i = 0; i < 300; i++) {\n      try { null.x; } catch (e) { c++; }\n    }\n    return c;\n  }\n  return r(n - 1) + 0;\n}\nconst keep = [];\nfor (let i = 0; i < 6; i++) keep.push(Buffer.alloc(50 << 20, 1));\nr(1000);";
    caso(&corpus, "s01", solo_b(lento_con_sensor, "", "console.log(\"listo\");").as_bytes());
    // 11.2e: s03, sin bug, un programa que nota el sensor (cuenta las llamadas a `prepareStackTrace`,
    // que el sensor dispara al pausar, como 23-i02 de la aceptacion) y en el que solo B pasa el tope.
    // La repeticion de B corre sin el sensor y da otra cuenta que A con el sensor; contra A sin el
    // sensor da lo mismo: iguales.
    let cuenta = "var calls = 0;\nError.prepareStackTrace = function () { calls++; return \"\"; };\ntry { null.x; } catch (e) {}";
    caso(&corpus, "s03", solo_b(&format!("{cuenta}\nconst keep = [];\nfor (let i = 0; i < 6; i++) keep.push(Buffer.alloc(50 << 20, 1));"), cuenta, "console.log(\"llamadas\", calls);").as_bytes());
    // 11.2f: h01 y h02, con el bug F de oxc (oxc#27098). El `toString` propio
    // de un error atrapado solo corre con el sensor (V8 formatea el stack al pausar). Con el bug, B con
    // el sensor reserva 400 MiB (pasa el tope); repetido sin el sensor no reserva y sale. h01: A sin el
    // sensor gira sin fin con la misma salida parcial que B; antes (70509f2) se tomaba como igual a B
    // y el bug se perdia: tiene que seguir en div/. h02: A sin el sensor termina y difiere de B: div/.
    let nota = "const n = \"abc\".substring(1, undefined).length;\nclass E extends Error { toString() { globalThis.v = 1; return \"E\"; } }\ntry { throw new E(\"x\"); } catch (e) {}\nconsole.log(\"hola\", typeof globalThis.v);\nif (n === 0) {\n  if (globalThis.v) { const b = []; for (let i = 0; i < 25; i++) b.push(Buffer.alloc(16 << 20, 1)); console.log(b.length); }\n  process.exit(0);\n}\n";
    caso(&corpus, "h01", format!("{nota}if (!globalThis.v) for (;;) {{}}\nconsole.log(\"fin\");\n").as_bytes());
    caso(&corpus, "h02", format!("{nota}console.log(\"fin\");\n").as_bytes());
    // 11.2f: x01 nota el sensor (el `toString` imprime solo con el
    // sensor) y tiene el bug F (oxc#27098); solo B reserva 400 MiB. A con el sensor, A sin el y B difieren: div/, y
    // el A guardado es el sin el sensor (A y B comparados sin el sensor). x02: B ademas gira sin fin:
    // la repeticion no termina y queda en div/ sin esa comparacion.
    let x = "class E extends Error { toString() { console.log(\"toString llamado\"); return \"E\"; } }\ntry { throw new E(\"x\"); } catch (e) {}\nconst n = \"abc\".substring(1, undefined).length;\nconst keep = [];\n";
    caso(&corpus, "x01", format!("{x}if (n === 0) for (let i = 0; i < 8; i++) keep.push(Buffer.alloc(50 << 20, 1));\nconsole.log(\"fin\", n);\n").as_bytes());
    caso(&corpus, "x02", format!("{x}if (n === 0) {{ for (let i = 0; i < 8; i++) keep.push(Buffer.alloc(50 << 20, 1)); for (;;) {{}} }}\nconsole.log(\"fin\", n);\n").as_bytes());
    // 11.2f (A sin el sensor corria una sola vez): d01, con el bug F (oxc#27098). Sin el sensor
    // el programa cuenta sus corridas en un archivo de afuera: la repeticion de B (1) y la primera de
    // A sin el sensor (2) imprimen lo mismo, la segunda (3) otra cosa. A sin el sensor no se reproduce:
    // inconcluso, sigue en div/ (con una sola corrida se tomaba como igual a B).
    let contador = b.join("contador");
    caso(
        &corpus,
        "d01",
        format!(
            "const fs = require(\"fs\");\nconst n = \"abc\".substring(1, undefined).length;\nlet sensor = false;\nclass E extends Error {{ toString() {{ sensor = true; return \"E\"; }} }}\ntry {{ throw new E(\"x\"); }} catch (e) {{}}\nif (sensor) {{\n  if (n === 0) {{ const b = []; for (let i = 0; i < 25; i++) b.push(Buffer.alloc(16 << 20, 1)); }}\n  console.log(\"con\", n);\n}} else {{\n  const p = {:?};\n  let c = 0;\n  try {{ c = Number(fs.readFileSync(p, \"utf8\")); }} catch (e) {{}}\n  c++;\n  fs.writeFileSync(p, String(c));\n  console.log(\"sin\", c <= 2 ? \"a\" : \"b\");\n}}\n",
            contador.to_str().unwrap()
        )
        .as_bytes(),
    );
    // 11.2g: g01, sin bug. El programa nota el sensor y sin el gira
    // sin fin (A y B igual); solo B pasa el tope. La repeticion de B (sin el sensor) no termina, A con el
    // sensor si: sigue en div/ (no se cambia ningun destino), pero la traza dice que A sin el sensor
    // tampoco termina: la divergencia la pone el sensor.
    let visto = "var visto = false;\nclass E extends Error { toString() { visto = true; return \"E\"; } }\ntry { throw new E(\"x\"); } catch (e) {}";
    caso(
        &corpus,
        "g01",
        solo_b(&format!("{visto}\nconst keep = [];\nfor (let i = 0; i < 6; i++) keep.push(Buffer.alloc(50 << 20, 1));"), visto, "if (!visto) for (;;) {}\nconsole.log(\"listo\");").as_bytes(),
    );
    // 11.2h: las corridas de A sin el sensor son auxiliares: si pasan
    // un tope, el paso queda inconcluso y el caso sigue en div/ (antes, `como_a`: recursos/). Los dos
    // con el bug F de oxc (oxc#27098) y un programa que nota el sensor; solo B reserva 300 MiB. g03: sin el sensor
    // imprime 256 KiB cada 500 ms sin fin: la repeticion de B (4 MiB de tope) da Timeout y A sin el
    // sensor (11.2g, 1 MiB) pasa el tope. g04: sin el sensor, el camino de A imprime sin fin y el de B
    // termina: la repeticion de B termina y difiere, y A sin el sensor (11.2f) pasa el tope.
    let visto_bug = "let visto = false;\nclass E extends Error { toString() { visto = true; return \"E\"; } }\ntry { throw new E(\"x\"); } catch (e) {}\nconst n = \"abc\".substring(1, undefined).length;\nconst keep = [];\nif (n === 0) for (let i = 0; i < 6; i++) keep.push(Buffer.alloc(50 << 20, 1));\n";
    let imprime = |cond: &str, kib: u32, ms: u32| format!("if ({cond}) {{\n  const s = \"z\".repeat({kib} * 1024);\n  const ia = new Int32Array(new SharedArrayBuffer(4));\n  for (;;) {{ process.stdout.write(s + \"\\n\"); Atomics.wait(ia, 0, 0, {ms}); }}\n}}\nconsole.log(\"fin\", n);\n");
    caso(&corpus, "g03", format!("{visto_bug}{}", imprime("!visto", 256, 500)).as_bytes());
    caso(&corpus, "g04", format!("{visto_bug}{}", imprime("!visto && n !== 0", 1024, 50)).as_bytes());
    // d02: como d01, pero la tercera corrida sin el sensor (la segunda de A sin el sensor) escribe 2 MiB
    // y pasa el tope de salida: inconcluso, sigue en div/.
    let contador2 = b.join("contador2");
    caso(
        &corpus,
        "d02",
        format!(
            "const fs = require(\"fs\");\nconst n = \"abc\".substring(1, undefined).length;\nlet sensor = false;\nclass E extends Error {{ toString() {{ sensor = true; return \"E\"; }} }}\ntry {{ throw new E(\"x\"); }} catch (e) {{}}\nif (sensor) {{\n  if (n === 0) {{ const b = []; for (let i = 0; i < 25; i++) b.push(Buffer.alloc(16 << 20, 1)); }}\n  console.log(\"con\", n);\n}} else {{\n  const p = {:?};\n  let c = 0;\n  try {{ c = Number(fs.readFileSync(p, \"utf8\")); }} catch (e) {{}}\n  c++;\n  fs.writeFileSync(p, String(c));\n  if (c >= 3) console.log(\"z\".repeat(2 << 20));\n  console.log(\"sin\", c);\n}}\n",
            contador2.to_str().unwrap()
        )
        .as_bytes(),
    );
    // 11.2i: el A repetido sin el sensor de I3 es auxiliar. Los
    // tres con el bug F de oxc (oxc#27098) y un programa que nota el sensor; el sensor de B se degrada (i01: B atrapa
    // 400 excepciones, `sensor_tope`; i3-tope e i3-colgado: B con el sensor gira, `sensor_caido`). i01 (de
    // la revision A): A sin el sensor imprime sin fin y pasa el tope de salida. i3-tope (B): A sin el
    // sensor reserva 400 MiB. i3-colgado (B): A sin el sensor no termina (y su salida parcial es la de B).
    // I3 queda inconcluso y el caso sigue en div/ (antes: recursos/, recursos/ e iguales).
    let nota_sensor = "let sensor = false;\nclass E extends Error { toString() { sensor = true; return \"E\"; } }\ntry { throw new E(\"x\"); } catch (e) {}\nconst n = \"abc\".substring(1, undefined).length;\n";
    caso(&corpus, "i01", format!("{nota_sensor}if (n === 0) for (let i = 0; i < 400; i++) {{ try {{ null.x; }} catch (e) {{}} }}\nif (!sensor) {{\n  const s = \"z\".repeat(1 << 20);\n  for (;;) process.stdout.write(s + \"\\n\");\n}}\nconsole.log(\"fin\", n);\n").as_bytes());
    caso(&corpus, "i3-tope", format!("{nota_sensor}if (sensor) {{\n  if (n === 0) for (;;) {{}}\n  console.log(\"con\", n);\n}} else {{\n  if (n !== 0) {{ const b = []; for (let i = 0; i < 25; i++) b.push(Buffer.alloc(16 << 20, 1)); }}\n  console.log(\"sin\", n);\n}}\n").as_bytes());
    caso(&corpus, "i3-colgado", format!("{nota_sensor}console.log(\"hola\");\nif (sensor) {{\n  if (n === 0) for (;;) {{}}\n  console.log(\"fin\");\n}} else {{\n  if (n === 0) process.exit(0);\n  for (;;) {{}}\n}}\n").as_bytes());
    // 11.2j: con I3 completo (B con el sensor gira y se
    // repite sin el; A se repite sin el), "A otra vez" repite el A auxiliar de I3. Las corridas sin el
    // sensor de A cuentan en un archivo de afuera. i3-tope2: la segunda reserva 400 MiB (pasa el tope);
    // i3-nodet: la segunda imprime otra cosa. Las dos quedan inconclusas y el caso sigue en div/ (antes:
    // recursos/ y no_determinista; i3-nodet ya pasaba en 2ff4d90).
    let i3_cuenta = |p: &Path, si_segunda: &str| {
        format!(
            "const fs = require(\"fs\");\n{nota_sensor}if (sensor) {{\n  if (n === 0) for (;;) {{}}\n  console.log(\"con\", n);\n}} else if (n === 0) {{\n  console.log(\"sin\", n);\n}} else {{\n  const p = {:?};\n  let c = 0;\n  try {{ c = Number(fs.readFileSync(p, \"utf8\")); }} catch (e) {{}}\n  c++;\n  fs.writeFileSync(p, String(c));\n  {si_segunda}\n}}\n",
            p.to_str().unwrap()
        )
    };
    caso(&corpus, "i3-tope2", i3_cuenta(&b.join("cuenta-i3-tope2"), "if (c >= 2) { const b = []; for (let i = 0; i < 25; i++) b.push(Buffer.alloc(16 << 20, 1)); }\n  console.log(\"sin\", n);").as_bytes());
    caso(&corpus, "i3-nodet", i3_cuenta(&b.join("cuenta-i3-nodet"), "console.log(c <= 1 ? \"sin 2\" : \"otra cosa\");").as_bytes());
    // i3-nodet2: ademas, con el sensor, A cuenta sus corridas en otro archivo e imprime la cuenta: el A
    // del caso tampoco se reproduce. Es no determinista de verdad: descartes (no_determinista).
    let con = b.join("cuenta-i3-nodet2-con");
    caso(
        &corpus,
        "i3-nodet2",
        i3_cuenta(&b.join("cuenta-i3-nodet2"), "console.log(c <= 1 ? \"sin 2\" : \"otra cosa\");")
            .replacen(
                "  console.log(\"con\", n);\n",
                &format!("  const q = {:?};\n  let k = 0;\n  try {{ k = Number(fs.readFileSync(q, \"utf8\")); }} catch (e) {{}}\n  k++;\n  fs.writeFileSync(q, String(k));\n  console.log(\"con\", n, k);\n", con.to_str().unwrap()),
                1,
            )
            .as_bytes(),
    );
    let out = b.join("out");
    let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--timeout-ms", "1000", "--mem-node", "300", "--max-salida", "1", "--no-cache", "--jobs", "1"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stats = stdout.lines().find(|l| l.starts_with("total=")).unwrap();
    for (c, n) in [("total", 21), ("iguales", 4), ("DIVERGENCIAS", 16), ("RECURSOS", 0), ("panics_del_fuzzer", 0)] {
        assert_eq!(campo(stats, c), n, "{c}: {stats}\n{}", err(&o));
    }
    let celda = out.join("oxc+carril_a+tal_cual");
    let meta = std::fs::read_to_string(celda.join("div/h01/meta.txt")).unwrap();
    assert!(meta.contains("A sin el sensor no termino solo (Timeout): inconcluso"), "{meta}");
    let meta = std::fs::read_to_string(celda.join("div/x01/meta.txt")).unwrap();
    assert!(meta.contains("A y B se comparan sin el sensor"), "{meta}");
    // Los re-chequeos tambien sin el sensor: A con mas pila (profundidad) da lo mismo que el A sin el
    // sensor. Con el runner con el sensor imprimiria "toString llamado" y el paso compararia otra cosa.
    assert!(meta.contains("  profundidad: no aplica (A con mas pila da lo mismo"), "{meta}");
    assert_eq!(std::fs::read_to_string(celda.join("div/x01/a.stdout")).unwrap(), "fin 2\n");
    assert!(celda.join("div/h02/meta.txt").is_file() && celda.join("div/x02/meta.txt").is_file());
    let meta = std::fs::read_to_string(celda.join("div/g01/meta.txt")).unwrap();
    assert!(meta.contains("LA DIVERGENCIA DEPENDE DEL SENSOR: A sin el sensor tampoco termina"), "{meta}");
    // x02: A sin el sensor termina (la repeticion de B no): esa nota, sin la otra.
    let meta = std::fs::read_to_string(celda.join("div/x02/meta.txt")).unwrap();
    assert!(meta.contains("A sin el sensor termina") && !meta.contains("DEPENDE DEL SENSOR"), "{meta}");
    for (g, cual) in [("g03", "A sin el sensor paso"), ("g04", "A sin el sensor paso"), ("d02", "A sin el sensor (segunda corrida) paso")] {
        let meta = std::fs::read_to_string(celda.join(format!("div/{g}/meta.txt"))).unwrap();
        assert!(meta.contains(&format!("{cual} un tope de node: inconcluso")), "{g}: {meta}");
    }
    for (i, por) in [("i01", "paso un tope de node"), ("i3-tope", "paso un tope de node"), ("i3-colgado", "no termino solo (Timeout)")] {
        let meta = std::fs::read_to_string(celda.join(format!("div/{i}/meta.txt"))).unwrap();
        assert!(meta.contains(&format!("A repetido sin el sensor {por}")) && meta.contains("inconcluso, A y B se comparan como estaban"), "{i}: {meta}");
    }
    let meta = std::fs::read_to_string(celda.join("div/i3-tope2/meta.txt")).unwrap();
    assert!(meta.contains("A otra vez (el auxiliar, sin el sensor) paso un tope de node: inconcluso, la divergencia sigue con el par de A y B sin el sensor"), "{meta}");
    let meta = std::fs::read_to_string(celda.join("div/i3-nodet/meta.txt")).unwrap();
    assert!(meta.contains("A otra vez (el auxiliar, sin el sensor) no se reproduce y el A del caso (con el sensor) si: inconcluso"), "{meta}");
    let descartes = std::fs::read_to_string(celda.join("descartes.tsv")).unwrap();
    assert!(descartes.contains("i3-nodet2/input.js\tno_determinista\t"), "{descartes}");
    let meta = std::fs::read_to_string(celda.join("div/d01/meta.txt")).unwrap();
    assert!(meta.contains("A sin el sensor no se reproduce (dos corridas distintas): inconcluso"), "{meta}");
    let meta = std::fs::read_to_string(celda.join("div/bucle-lento/meta.txt")).unwrap();
    assert!(meta.contains("kind: Timeout\n") && meta.contains("antes de terminar; queda el Timeout"), "{meta}");
    let meta = std::fs::read_to_string(celda.join("div/mayor-gira/meta.txt")).unwrap();
    assert!(meta.contains("kind: ExitStatus\n") && meta.contains("no termino (Timeout): sigue la divergencia con el B cortado"), "{meta}");
    // 11.2e (V2-H1 y B-F2): la repeticion que no termina ocupa el turno un solo timeout largo (con el
    // reintento sin el sensor adentro del turno, el doble).
    let num = |tras: &str| -> u128 { meta.split(tras).nth(1).and_then(|s| s.split(' ').next()).and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("sin `{tras}`: {meta}")) };
    let (largo, turno) = (num("sin el sensor, timeout "), num("; turno "));
    assert!(turno < largo + 1500, "turno {turno} ms con un timeout largo de {largo} ms");
    // 11.2c: el B cortado que se guarda es un prefijo con la marca.
    let b_out = std::fs::read_to_string(celda.join("div/mayor-gira/b.stdout")).unwrap();
    assert!(b_out.len() < 70 * 1024 && b_out.contains("[... difuzz recorto "), "{}", b_out.len());
    std::fs::remove_dir_all(&b).unwrap();
}

/// 11.2c (H1 = B-N3, H3: MUT-c y MUT-d): un A que pasa un tope al confirmar una
/// divergencia va a recursos/, como cualquier A que pasa un tope. `marca` (sin bug; A depende de un
/// archivo afuera) da "chico" la primera vez y "grande" (400 MiB) despues: el B y el "A otra vez"
/// de la primera corrida ya ven la marca. Corrida 1 (`--cache`): A ("chico") entra en la cache, el "A
/// otra vez" pasa el tope (con MUT-d, iguales). Corrida 2: A sale de la cache y el A fresco que la
/// confirma pasa el tope (con MUT-c, iguales). Y la cache con otros topes (c01 y c06 de la revision
/// A): grabada con `--mem-node 2048` y `--max-salida 16`, no sirve con 300 y 1 (A corre en fresco y
/// va a recursos/; con la entrada, B pasaba el tope, se repetia con los mayores y daba iguales).
#[test]
fn a_que_pasa_el_tope_al_confirmar_va_a_recursos() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("a-confirma-tope");
    let corpus = b.join("corpus");
    let marca = b.join("marca");
    caso(
        &corpus,
        "marca",
        format!(
            "const fs = require(\"fs\");\nconst m = {:?};\nconst grande = fs.existsSync(m);\nif (!grande) fs.writeFileSync(m, \"\");\nconst keep = [];\nif (grande) for (let i = 0; i < 8; i++) keep.push(Buffer.alloc(50 << 20, 1));\nconsole.log(grande ? \"grande\" : \"chico\", keep.length);\n",
            marca.to_str().unwrap()
        )
        .as_bytes(),
    );
    let cache = b.join("cache");
    let correr = |out: &Path, corpus: &Path, cache: &Path, topes: &[&str]| {
        let mut args = vec![corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--cache", cache.to_str().unwrap(), "--jobs", "1"];
        args.extend_from_slice(topes);
        let o = difuzz(&b, &args);
        assert_eq!(rc(&o), Some(0), "{}", err(&o));
        String::from_utf8_lossy(&o.stdout).lines().find(|l| l.starts_with("total=")).unwrap().to_string()
    };
    for (i, quien, hits) in [(1, "el original (A) otra vez, al confirmar una divergencia (no determinismo)", 0), (2, "el original (A) en fresco, al confirmar una divergencia con el A de la cache", 1)] {
        let out = b.join(format!("out-{i}"));
        let stats = correr(&out, &corpus, &cache, &["--mem-node", "300"]);
        for (c, n) in [("total", 1), ("iguales", 0), ("DIVERGENCIAS", 0), ("RECURSOS", 1), ("node_memoria", 1), ("cache_a_hits", hits)] {
            assert_eq!(campo(&stats, c), n, "corrida {i}, {c}: {stats}");
        }
        let meta = std::fs::read_to_string(out.join("oxc+carril_a+tal_cual/recursos/marca/meta.txt")).unwrap();
        assert!(meta.contains(&format!("ejecucion: {quien}\n")) && meta.contains("causa: node_memoria\n"), "corrida {i}: {meta}");
    }

    let corpus_c = b.join("corpus-c");
    caso(&corpus_c, "pesado", b"const keep = [];\nfor (let i = 0; i < 8; i++) keep.push(Buffer.alloc(50 << 20, 1));\nconsole.log(keep.length);\n");
    caso(&corpus_c, "salida", b"const s = \"q\".repeat(1023);\nfor (let i = 0; i < 1100; i++) console.log(s);\n");
    let cache_c = b.join("cache-c");
    let stats = correr(&b.join("out-c1"), &corpus_c, &cache_c, &["--mem-node", "2048"]);
    assert_eq!((campo(&stats, "iguales"), campo(&stats, "RECURSOS")), (2, 0), "{stats}");
    let out = b.join("out-c2");
    let stats = correr(&out, &corpus_c, &cache_c, &["--mem-node", "300", "--max-salida", "1"]);
    for (c, n) in [("total", 2), ("iguales", 0), ("RECURSOS", 2), ("node_memoria", 1), ("node_salida", 1), ("cache_a_hits", 0)] {
        assert_eq!(campo(&stats, c), n, "{c}: {stats}");
    }
    let meta = std::fs::read_to_string(out.join("oxc+carril_a+tal_cual/recursos/pesado/meta.txt")).unwrap();
    assert!(meta.contains("ejecucion: el original (A) del caso, en la pasada cruda\n"), "{meta}");

    // Lo que dice meta.txt de un A de la cache: B (bug F de oxc, oxc#27098) reserva sin fin.
    // Sin cache: `a_cache: miss`. Con la entrada (grabada con los mismos topes): el A fresco la
    // confirma (`hit (confirmado con una corrida fresca)`) y la traza dice de donde sale "A no".
    let corpus_h = b.join("corpus-h");
    caso(&corpus_h, "solo-b", b"const n = \"abc\".substring(1, undefined).length;\nconst keep = [];\nif (n === 0) for (;;) keep.push(Buffer.alloc(32 << 20, 1));\nconsole.log(n);\n");
    let cache_h = b.join("cache-h");
    for (i, a_cache, a_no) in [(1, "a_cache: miss\n", "y A no; repetido"), (2, "a_cache: hit (confirmado con una corrida fresca)\n", "y A no (A de la cache, grabado con topes iguales o menores; se confirma en fresco abajo); repetido")] {
        let out = b.join(format!("out-h{i}"));
        let stats = correr(&out, &corpus_h, &cache_h, &["--mem-node", "300"]);
        assert_eq!((campo(&stats, "DIVERGENCIAS"), campo(&stats, "cache_a_hits")), (1, i - 1), "{stats}");
        let meta = std::fs::read_to_string(out.join("oxc+carril_a+tal_cual/div/solo-b/meta.txt")).unwrap();
        assert!(meta.contains("kind: ExitStatus\n") && meta.contains(a_cache) && meta.contains(a_no), "corrida {i}: {meta}");
    }
    std::fs::remove_dir_all(&b).unwrap();
}

/// 11.2c: las repeticiones de B con los topes mayores (el doble de RSS) van
/// de a una en todo difuzz. Dos casos con `--jobs 2` cuyo B (sin bug, mas corto) reserva 400 MiB:
/// los dos pasan el tope de 300 casi a la vez y se repiten con 600. En la repeticion, B toma un
/// archivo exclusivo (`wx`) durante 3 s: si otra repeticion lo tiene, imprime `solapado` y diverge.
/// Con el turno, iguales los dos; sin el (las dos a la vez, `jobs` x 2 x tope), uno va a div/.
/// 11.2k: I3 con el A del caso leido de la cache. El programa tiene
/// el bug F de oxc (oxc#27098) y nota el sensor; con el sensor imprime una "version" que se lee de un archivo de
/// afuera (cambia entre corridas: la entrada de la cache queda vieja); sin el sensor, A alterna su
/// salida (el A auxiliar de I3 no se reproduce). Corrida 1: div/. Corrida 2, misma cache: el A del caso
/// fresco difiere de la entrada; antes eso mandaba un bug real a no_determinista. Ahora: lapida, otra
/// corrida fresca que da lo mismo, inconcluso y div/.
#[test]
fn i3_con_una_entrada_vieja_de_la_cache_sigue_en_div() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("i3-cache");
    let corpus = b.join("corpus");
    let (version, contador, cache) = (b.join("version.txt"), b.join("contador"), b.join("cache"));
    caso(
        &corpus,
        "i3-cache",
        format!(
            "const fs = require(\"fs\");\nconst n = \"abc\".substring(1, undefined).length;\nlet sensor = false;\nclass E extends Error {{ toString() {{ sensor = true; return \"E\"; }} }}\ntry {{ throw new E(\"x\"); }} catch (e) {{}}\nconst ver = fs.readFileSync({:?}, \"utf8\").trim();\nif (sensor) {{\n  if (n === 0) for (;;) {{}}\n  console.log(\"con\", n, ver);\n}} else if (n === 0) {{\n  console.log(\"sin\", n);\n}} else {{\n  const p = {:?};\n  let c = 0;\n  try {{ c = Number(fs.readFileSync(p, \"utf8\")); }} catch (e) {{}}\n  c++;\n  fs.writeFileSync(p, String(c));\n  console.log(c % 2 ? \"sin a\" : \"sin b\");\n}}\n",
            version.to_str().unwrap(),
            contador.to_str().unwrap()
        )
        .as_bytes(),
    );
    for (i, ver, hits) in [(1, "v1", 0), (2, "v2", 1)] {
        std::fs::write(&version, ver).unwrap();
        let out = b.join(format!("out-{i}"));
        let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--cache", cache.to_str().unwrap(), "--timeout-ms", "1000", "--jobs", "1"]);
        assert_eq!(rc(&o), Some(0), "{}", err(&o));
        let stdout = String::from_utf8_lossy(&o.stdout);
        let stats = stdout.lines().find(|l| l.starts_with("total=")).unwrap();
        for (c, n) in [("total", 1), ("DIVERGENCIAS", 1), ("no_determinista", 0), ("cache_a_hits", hits)] {
            assert_eq!(campo(stats, c), n, "corrida {i}, {c}: {stats}");
        }
        let meta = std::fs::read_to_string(out.join("oxc+carril_a+tal_cual/div/i3-cache/meta.txt")).unwrap();
        assert!(meta.contains("A y B comparados sin el sensor"), "corrida {i}: {meta}");
        if i == 2 {
            assert!(meta.contains("salio de la cache y una corrida fresca dio otra cosa: lapida"), "{meta}");
        }
    }
    std::fs::remove_dir_all(&b).unwrap();
}

#[test]
fn una_repeticion_con_topes_mayores_a_la_vez() {
    if !test_util::requiere_node() {
        return;
    }
    let b = base("turno-mayor");
    let corpus = b.join("corpus");
    let candado = b.join("en-curso");
    let cuerpo_b = format!(
        "const keep = [];\nfor (let i = 0; i < 8; i++) keep.push(Buffer.alloc(50 << 20, 1));\nconst fs = require(\"fs\");\nlet fd;\ntry {{ fd = fs.openSync({:?}, \"wx\"); }} catch (e) {{ console.log(\"solapado\"); }}\nif (fd !== undefined) {{ Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 3000); fs.closeSync(fd); fs.unlinkSync({:?}); }}",
        candado.to_str().unwrap(),
        candado.to_str().unwrap()
    );
    for c in ["s1", "s2"] {
        caso(&corpus, c, solo_b(&cuerpo_b, "", &format!("console.log(\"listo\", {c:?});")).as_bytes());
    }
    let out = b.join("out");
    let o = difuzz(&b, &[corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--mem-node", "300", "--no-cache", "--jobs", "2"]);
    assert_eq!(rc(&o), Some(0), "{}", err(&o));
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stats = stdout.lines().find(|l| l.starts_with("total=")).unwrap();
    for (c, n) in [("total", 2), ("iguales", 2), ("DIVERGENCIAS", 0), ("RECURSOS", 0)] {
        assert_eq!(campo(stats, c), n, "{c}: {stats}\n{}", err(&o));
    }
    assert!(!candado.exists());
    std::fs::remove_dir_all(&b).unwrap();
}
