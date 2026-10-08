//! Mejora 7: `__difuzzObserveInvocar` del preload, corrido en node de verdad.
//! Sin `node` en el PATH el test no tiene que fallar: avisa y sale.

use std::process::Command;

use crate::runner::PRELOAD;

/// Corre `js` con el preload y devuelve (exit code, stdout).
fn run_node(tag: &str, js: &str) -> Option<(i32, String)> {
    let dir = std::env::temp_dir().join(format!("difuzz-invocar-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let preload = dir.join("preload.cjs");
    let script = dir.join("caso.cjs");
    std::fs::write(&preload, PRELOAD).ok()?;
    std::fs::write(&script, js).ok()?;
    let out = Command::new("node")
        .arg("--no-warnings")
        .arg("--require")
        .arg(&preload)
        .arg(&script)
        .env("TZ", "UTC")
        .output();
    let _ = std::fs::remove_dir_all(&dir);
    let out = out.ok()?;
    Some((out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned()))
}

#[test]
fn invoca_funciones_observadas() {
    let src = "\
function f() { return 42; }
function g() { throw new TypeError('n is not a function'); }
async function h() { await null; return [1]; }
async function r() { throw new RangeError('x'); }
class C {}
var k = 7;
function e() { return function inner() {}; }
function t() { return this === undefined ? 'u' : typeof this; }
";
    let Some(epilogo) = crate::minify::instrument(src, true, crate::minify::SourceKind::Script) else {
        panic!("instrument no observo nada");
    };
    assert!(epilogo.ends_with("__difuzzObserveInvocar([\"f\", f], [\"g\", g], [\"h\", h], [\"r\", r], [\"C\", C], [\"k\", k], [\"e\", e], [\"t\", t]);\n"));
    let Some((code, stdout)) = run_node("a", &epilogo) else {
        eprintln!("sin node en el PATH: test salteado");
        return;
    };
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        stdout,
        "\
f = fn
g = fn
h = fn
r = fn
C = fn
k = 7
e = fn
t = fn
f() = 42
g() throws TypeError
h() = promesa
r() = promesa
C() throws TypeError
e() = fn
t() = \"object\"
!dz promesa h() = [1]
!dz promesa r() throws RangeError
"
    );
}

#[test]
fn sin_invocar_la_salida_no_cambia() {
    let src = "function f() { return 42; }\nvar k = 7;\n";
    let Some(epilogo) = crate::minify::instrument(src, false, crate::minify::SourceKind::Script) else {
        panic!("instrument no observo nada");
    };
    assert!(epilogo.ends_with("__difuzzObserve([\"f\", f], [\"k\", k]);\n"));
    let Some((code, stdout)) = run_node("b", &epilogo) else {
        eprintln!("sin node en el PATH: test salteado");
        return;
    };
    assert_eq!((code, stdout.as_str()), (0, "f = fn\nk = 7\n"));
}

/// M6: un Error observado o devuelto se serializa solo con su tipo: el mensaje
/// trae nombres locales que el minifier renombra.
#[test]
fn error_observado_sin_mensaje() {
    let src = "var e = new TypeError('callback is not a function');\nclass Mio extends RangeError {}\nvar m = new Mio('x');\nfunction f() { return new Error('local'); }\n";
    let epilogo = crate::minify::instrument(src, true, crate::minify::SourceKind::Script).expect("observa e, Mio, m, f");
    let Some((code, stdout)) = run_node("m6", &epilogo) else {
        eprintln!("sin node en el PATH: test salteado");
        return;
    };
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(stdout, "e = Error(TypeError)\nMio = fn\nm = Error(RangeError)\nf = fn\nMio() throws TypeError\nf() = Error(Error)\n");
}

/// M36: un CJS sin bindings top-level se observa por `module.exports`, y con
/// invocar se llama a lo exportado. Tambien con `en_funcion` (el `module` del
/// envoltorio CommonJS sigue a la vista).
#[test]
fn observa_module_exports() {
    use crate::minify::{instrument, Forma, SourceKind};
    let src = "module.exports = { a: 1, f() { return 2 } };\n";
    assert!(instrument(src, false, SourceKind::Module).is_none());
    let epi = instrument(src, false, SourceKind::Script).expect("observa module.exports");
    assert!(epi.ends_with("__difuzzObserveCjs(module.exports, false);\n"), "{epi}");
    let Some((code, stdout)) = run_node("m36", &epi) else {
        eprintln!("sin node en el PATH: test salteado");
        return;
    };
    assert_eq!((code, stdout.as_str()), (0, "module.exports = {\"a\": 1, \"f\": fn}\n"));
    let inv = Forma::EnFuncion.apply(&instrument(src, true, SourceKind::Script).unwrap());
    let (code, stdout) = run_node("m36i", &inv).unwrap();
    assert_eq!((code, stdout.as_str()), (0, "module.exports = {\"a\": 1, \"f\": fn}\nmodule.exports.f() = 2\n"));
    // Una UMD que exporta una funcion: se la llama a ella y a sus metodos.
    let umd = "(function (root, factory) { typeof module === 'object' ? module.exports = factory() : root.x = factory(); })(this, function () { function x() { return 7; } x.y = function () { return 8; }; return x; });\n";
    let (code, stdout) = run_node("m36u", &instrument(umd, true, SourceKind::Script).unwrap()).unwrap();
    assert_eq!((code, stdout.as_str()), (0, "module.exports = fn\nmodule.exports() = 7\nmodule.exports.y() = 8\n"));
    // Sin mencionar module/exports, nada cambia; `modules` no es `module`.
    assert!(instrument("modules.x = 1;\n", false, SourceKind::Script).is_none());
}
