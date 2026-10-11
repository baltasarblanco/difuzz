// 8.3 positivo (d2-hallazgos h1b/04): V8 escribe el nombre del parametro en el mensaje y el
// mangle lo cambia (`cb is not a function` / `e is not a function`). known/ por `mensaje_nombre`.
function run(cb) {
  try {
    cb();
  } catch (e) {
    console.log(e.message);
  }
}
run();
