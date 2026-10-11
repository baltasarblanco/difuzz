// 9.3 negativo (M2): el mensaje con una llamada (`cb(...) is not a function` / `e(...)`) y, en otra
// linea, el bug de 9460 (IIFE con rest y default con efecto borrada). Tiene que quedar en div/.
function run(cb) {
  try {
    cb(1)();
  } catch (e) {
    console.log(e.message);
  }
}
run(function () {
  return 1;
});
function rec(v) { console.log(v); }
(function (...[value = rec("rest")]) {})();
console.log("fin");
