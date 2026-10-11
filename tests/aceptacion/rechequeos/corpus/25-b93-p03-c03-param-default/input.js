// 9.3 (M2, c03 de rev-b8-A): `f()()` con `f` un parametro con default anonimo. V8 escribe
// `f(...) is not a function`. En core_defaults el renombre es correcto y el `.name` no se promete:
// known/ por `mensaje_nombre`. En carril_a (keep_names) oxc 0.153.0 manglea `f` igual (el bug de
// keep_names de H1): el re-chequeo da Inconcluso y queda en div/.
function run(cb, f = () => cb) {
  var cb = 2;
  try {
    f()();
  } catch (e) {
    console.log(e.message);
  }
  try {
    cb();
  } catch (e) {
    console.log(e.message);
  }
}
run();
