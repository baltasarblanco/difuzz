// 9.1, control: los mismos `[Function: x]` y `[class X]` desde formas que keep_names de oxc 0.153.0 cubre
// (declarador, declaracion, expresion con nombre). En carril_a oxc conserva los nombres: igual. En core_defaults
// (sin keep_names) el mangle los cambia legitimamente y normalize_names los sigue borrando: igual.
function run(p) {
  const handler = () => {};
  function named() {}
  class Klass {}
  const expr = function inner() {};
  console.log(handler, named, Klass, expr, handler.bind(null), [handler], { k: Klass });
  return p;
}
run();
