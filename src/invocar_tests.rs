//! Mejora 7: `__difuzzObserveInvocar` del preload, corrido en node de verdad.
//! Sin `node` en el PATH los tests fallan (salvo `DIFUZZ_SIN_NODE=1`: ver
//! test_util.rs).

/// Corre `js` con el preload y devuelve (exit code, stdout). `None` si node no
/// se pudo lanzar. R1-H17: por el `Runner` real (entorno minimo, cwd vacio,
/// grupo de procesos, timeout, canal de control), no un `node` a mano.
fn run_node(tag: &str, js: &str) -> Option<(i32, String)> {
    use crate::runner::{Preload, Runner, Status};
    // Un id por llamada (registro de test_util): el runner arma con el id sus
    // rutas (script, ctl, cwd) y los tests corren en paralelo.
    let preload = Preload::write_tagged(&format!("invocar-{tag}")).ok()?;
    let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
    let e = r.run(js);
    match e.status {
        Status::Exited(code) => Some((code, e.stdout)),
        Status::Failed(f) => {
            eprintln!("runner: {f}");
            None
        }
        Status::Timeout | Status::Signal => Some((-1, e.stdout)),
    }
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
    if !crate::test_util::requiere_node() {
        return;
    }
    let Some((code, stdout)) = run_node("a", &epilogo) else {
        panic!("run_node({:?}) no pudo correr node", "a");
    };
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        stdout,
        "\
f = [Function: f]
g = [Function: g]
h = [Function: h]
r = [Function: r]
C = [Function: C]
k = 7
e = [Function: e]
t = [Function: t]
f() = 42
g() throws TypeError
h() = promesa
r() = promesa
C() throws TypeError
e() = [Function: inner]
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
    if !crate::test_util::requiere_node() {
        return;
    }
    let Some((code, stdout)) = run_node("b", &epilogo) else {
        panic!("run_node({:?}) no pudo correr node", "b");
    };
    assert_eq!((code, stdout.as_str()), (0, "f = [Function: f]\nk = 7\n"));
}

/// M6: un Error observado o devuelto se serializa solo con su tipo: el mensaje
/// trae nombres locales que el minifier renombra.
#[test]
fn error_observado_sin_mensaje() {
    let src = "var e = new TypeError('callback is not a function');\nclass Mio extends RangeError {}\nvar m = new Mio('x');\nfunction f() { return new Error('local'); }\n";
    let epilogo = crate::minify::instrument(src, true, crate::minify::SourceKind::Script).expect("observa e, Mio, m, f");
    if !crate::test_util::requiere_node() {
        return;
    }
    let Some((code, stdout)) = run_node("m6", &epilogo) else {
        panic!("run_node({:?}) no pudo correr node", "m6");
    };
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(stdout, "e = Error(TypeError)\nMio = [Function: Mio]\nm = Error(RangeError)\nf = [Function: f]\nMio() throws TypeError\nf() = Error(Error)\n");
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
    if !crate::test_util::requiere_node() {
        return;
    }
    let Some((code, stdout)) = run_node("m31b", &epi) else {
        panic!("run_node({:?}) no pudo correr node", "m31b");
    };
    // (Se invoca la del programa como un binding mas: imprime `mia`.)
    assert_eq!((code, stdout.as_str()), (0, "__difuzzObserveInvocar = [Function: __difuzzObserveInvocar]\nk = 1\nf = [Function: f]\nmia\n__difuzzObserveInvocar() = undefined\nf() = 2\n"), "{epi}");
    // Lo mismo dentro de una funcion (forma en_funcion).
    let (code, stdout) = run_node("m31b2", &Forma::EnFuncion.apply(&epi)).unwrap();
    assert_eq!(code, 0);
    assert!(stdout.ends_with("f() = 2\n"), "{stdout}");
    // `globalThis` declarado (sin la otra colision): nombre suelto.
    let g = instrument("var globalThis = 1;\nfunction f() { return 2; }\n", true, SourceKind::Script).unwrap();
    assert!(g.contains("\n__difuzzObserveInvocar([\"globalThis\", globalThis], [\"f\", f]);\n"), "{g}");
    let (code, stdout) = run_node("m31b3", &g).unwrap();
    assert_eq!((code, stdout.as_str()), (0, "globalThis = 1\nf = [Function: f]\nf() = 2\n"));
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
    if !crate::test_util::requiere_node() {
        return;
    }
    let Some((code, stdout)) = run_node("m31c", &epi) else {
        panic!("run_node({:?}) no pudo correr node", "m31c");
    };
    assert_eq!(
        (code, stdout.as_str()),
        (0, "k = 1\no = {\"n\": 0}\ns = \"x\"\ninc = [Function: inc]\ninc() = undefined\nk = 2 (tras invocar)\no = {\"n\": 5} (tras invocar)\n")
    );
    // CJS: module.exports despues de invocar.
    let cjs = "module.exports = { n: 0, f() { module.exports.n = 3; } };\n";
    let (code, stdout) = run_node("m31c2", &instrument(cjs, true, SourceKind::Script).unwrap()).unwrap();
    assert_eq!(
        (code, stdout.as_str()),
        (0, "module.exports = {\"n\": 0, \"f\": [Function: f]}\nmodule.exports.f() = undefined\nmodule.exports = {\"n\": 3, \"f\": [Function: f]} (tras invocar)\n")
    );
}

/// M31 (c) en el carril B: los exports son bindings vivos y se re-observan.
#[test]
fn reobserva_exports_despues_de_invocar() {
    use crate::minify::SourceKind;
    use crate::runner::{Preload, Runner, Status};
    if !crate::test_util::requiere_node() {
        return;
    }
    let preload = Preload::write_tagged("m31x").unwrap();
    let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20)).with_kind(SourceKind::Module).with_invocar(true);
    let e = r.observing().run("export let n = 0;\nexport function f() { n = 4; }\nexport const c = 1;\n");
    assert_eq!(e.status, Status::Exited(0), "{}", e.stderr);
    assert_eq!(e.stdout, "export c = 1\nexport f = [Function: f]\nexport n = 0\nexport f() = undefined\nexport n = 4 (tras invocar)\n");
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
    if !crate::test_util::requiere_node() {
        return;
    }
    let Some((code, stdout)) = run_node("m36", &epi) else {
        panic!("run_node({:?}) no pudo correr node", "m36");
    };
    assert_eq!((code, stdout.as_str()), (0, "module.exports = {\"a\": 1, \"f\": [Function: f]}\n"));
    let inv = Forma::EnFuncion.apply(&instrument(src, true, SourceKind::Script).unwrap());
    let (code, stdout) = run_node("m36i", &inv).unwrap();
    assert_eq!((code, stdout.as_str()), (0, "module.exports = {\"a\": 1, \"f\": [Function: f]}\nmodule.exports.f() = 2\n"));
    // Una UMD que exporta una funcion: se la llama a ella y a sus metodos.
    let umd = "(function (root, factory) { typeof module === 'object' ? module.exports = factory() : root.x = factory(); })(this, function () { function x() { return 7; } x.y = function () { return 8; }; return x; });\n";
    let (code, stdout) = run_node("m36u", &instrument(umd, true, SourceKind::Script).unwrap()).unwrap();
    assert_eq!((code, stdout.as_str()), (0, "module.exports = [Function: x]\nmodule.exports() = 7\nmodule.exports.y() = 8\n"));
    // Sin mencionar module/exports, nada cambia; `modules` no es `module`.
    assert!(instrument("modules.x = 1;\n", false, SourceKind::Script).is_none());
}
