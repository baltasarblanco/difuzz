// 8.3 negativo (d2-hallazgos h1-adv/01): el mensaje con el nombre mangleado y, en otra linea,
// el bug de 9460 (IIFE con rest y default con efecto borrada). Tiene que quedar en div/.
function run(cb) {
  try {
    cb();
  } catch (e) {
    console.log(e.message);
  }
}
run();
function rec(v) { console.log(v); }
(function (...[value = rec("rest")]) {})();
console.log("fin");
