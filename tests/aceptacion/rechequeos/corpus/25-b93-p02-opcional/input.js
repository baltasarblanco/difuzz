// 9.3 positivo (M2): el acceso opcional como lo escribe V8 (`o?.g is not a function`) y una
// llamada sobre el resultado (`o.f(...)?.g`), con el parametro mangleado. known/ por
// `mensaje_nombre`.
function run(o) {
  try {
    o?.g();
  } catch (e) {
    console.log(e.message);
  }
  try {
    o.f()?.g();
  } catch (e) {
    console.log(e.message);
  }
}
run({
  f: function () {
    return {};
  },
});
