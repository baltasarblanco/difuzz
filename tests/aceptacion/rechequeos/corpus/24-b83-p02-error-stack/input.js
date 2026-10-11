// 8.3 positivo (d2-hallazgos h1/01, la forma de hono sse.js): el programa imprime el error con
// su stack. Cambian el nombre del mensaje y, sin keep_names (core_defaults), el del marco.
function run(cb) {
  try {
    cb();
  } catch (e) {
    console.error(e);
  }
}
run();
