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
    let Some(epilogo) = crate::minify::instrument(src, true) else {
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
    let Some(epilogo) = crate::minify::instrument(src, false) else {
        panic!("instrument no observo nada");
    };
    assert!(epilogo.ends_with("__difuzzObserve([\"f\", f], [\"k\", k]);\n"));
    let Some((code, stdout)) = run_node("b", &epilogo) else {
        eprintln!("sin node en el PATH: test salteado");
        return;
    };
    assert_eq!((code, stdout.as_str()), (0, "f = fn\nk = 7\n"));
}
