// 8.3 negativo (d2-hallazgos h1-adv/02): el mensaje sale del codigo que el bug de 9460 deja de
// correr (A imprime `cb is not a function`, B nada). Tiene que quedar en div/.
function run(cb) {
  try {
    (function (...[value = cb()]) {})();
  } catch (e) {
    console.log(e.message);
  }
  console.log("fin");
}
run();
