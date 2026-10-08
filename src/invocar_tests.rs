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
    // M31: sin colision, el nombre suelto de siempre, y despues la re-observacion
    // solo de los var/let/const.
    assert!(epilogo.ends_with("\n__difuzzObserveInvocar([\"f\", f], [\"g\", g], [\"h\", h], [\"r\", r], [\"C\", C], [\"k\", k], [\"e\", e], [\"t\", t]);\n__difuzzObserveDespues([\"k\", k]);\n"), "{epilogo}");
    // Sin var/let/const, ni la llamada (no cambia lo que el minifier puede borrar).
    let solo_fn = crate::minify::instrument("function foo() { var { x } = foo; }\n", true, crate::minify::SourceKind::Script).unwrap();
    assert!(!solo_fn.contains("Despues"), "{solo_fn}");
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

/// M31 (b): un programa que declara `__difuzzObserveInvocar` ya no sombrea la
/// del preload (se llama via globalThis; en CJS el binding es local). Si
/// declara `globalThis`, se cae al nombre suelto; si declara los dos, sin invocar.
#[test]
fn invocar_no_se_sombrea() {
    use crate::minify::{instrument, Forma, SourceKind};
    let src = "function __difuzzObserveInvocar() { console.log('mia'); }\nvar k = 1;\nfunction f() { return 2; }\n";
    let epi = instrument(src, true, SourceKind::Script).unwrap();
    assert!(epi.contains("\nglobalThis[\"__difuzzObserveInvocar\"]([\"__difuzzObserveInvocar\", __difuzzObserveInvocar], "), "{epi}");
    let Some((code, stdout)) = run_node("m31b", &epi) else {
        eprintln!("sin node en el PATH: test salteado");
        return;
    };
    // (Se invoca la del programa como un binding mas: imprime `mia`.)
    assert_eq!((code, stdout.as_str()), (0, "__difuzzObserveInvocar = fn\nk = 1\nf = fn\nmia\n__difuzzObserveInvocar() = undefined\nf() = 2\n"), "{epi}");
    // Lo mismo dentro de una funcion (forma en_funcion).
    let (code, stdout) = run_node("m31b2", &Forma::EnFuncion.apply(&epi)).unwrap();
    assert_eq!(code, 0);
    assert!(stdout.ends_with("f() = 2\n"), "{stdout}");
    // `globalThis` declarado (sin la otra colision): nombre suelto.
    let g = instrument("var globalThis = 1;\nfunction f() { return 2; }\n", true, SourceKind::Script).unwrap();
    assert!(g.contains("\n__difuzzObserveInvocar([\"globalThis\", globalThis], [\"f\", f]);\n"), "{g}");
    let (code, stdout) = run_node("m31b3", &g).unwrap();
    assert_eq!((code, stdout.as_str()), (0, "globalThis = 1\nf = fn\nf() = 2\n"));
    // Los dos declarados: no hay por donde invocar.
    let d = instrument("var globalThis = 1;\nfunction __difuzzObserveInvocar() {}\n", true, SourceKind::Script).unwrap();
    assert!(d.ends_with("\n__difuzzObserve([\"globalThis\", globalThis], [\"__difuzzObserveInvocar\", __difuzzObserveInvocar]);\n"), "{d}");
}

/// M31 (c): lo que cambian las funciones invocadas se re-observa.
#[test]
fn reobserva_despues_de_invocar() {
    use crate::minify::{instrument, SourceKind};
    let src = "var k = 1;\nvar o = { n: 0 };\nvar s = 'x';\nfunction inc() { k++; o.n = 5; }\n";
    let epi = instrument(src, true, SourceKind::Script).unwrap();
    let Some((code, stdout)) = run_node("m31c", &epi) else {
        eprintln!("sin node en el PATH: test salteado");
        return;
    };
    assert_eq!(
        (code, stdout.as_str()),
        (0, "k = 1\no = {\"n\": 0}\ns = \"x\"\ninc = fn\ninc() = undefined\nk = 2 (tras invocar)\no = {\"n\": 5} (tras invocar)\n")
    );
    // CJS: module.exports despues de invocar.
    let cjs = "module.exports = { n: 0, f() { module.exports.n = 3; } };\n";
    let (code, stdout) = run_node("m31c2", &instrument(cjs, true, SourceKind::Script).unwrap()).unwrap();
    assert_eq!(
        (code, stdout.as_str()),
        (0, "module.exports = {\"n\": 0, \"f\": fn}\nmodule.exports.f() = undefined\nmodule.exports = {\"n\": 3, \"f\": fn} (tras invocar)\n")
    );
}

/// M31 (c) en el carril B: los exports son bindings vivos y se re-observan.
#[test]
fn reobserva_exports_despues_de_invocar() {
    use crate::minify::SourceKind;
    use crate::runner::{Preload, Runner, Status};
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("sin node en el PATH: test salteado");
        return;
    }
    let preload = Preload::write_tagged("m31x").unwrap();
    // Id propio: el 9989 lo usa tambien cache.rs (mismo cwd y script en paralelo).
    let r = Runner::new(&preload, 9982, std::time::Duration::from_secs(20)).with_kind(SourceKind::Module).with_invocar(true);
    let e = r.observing().run("export let n = 0;\nexport function f() { n = 4; }\nexport const c = 1;\n");
    assert_eq!(e.status, Status::Exited(0), "{}", e.stderr);
    assert_eq!(e.stdout, "export c = 1\nexport f = fn\nexport n = 0\nexport f() = undefined\nexport n = 4 (tras invocar)\n");
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
