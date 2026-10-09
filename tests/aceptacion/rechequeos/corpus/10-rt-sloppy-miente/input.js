function rec(v) { console.log(v); }
function f() { return this; }
if (f() === undefined) {
  console.log("estricto");
} else {
  (function (...[value = rec("rest")]) {})();
}
console.log("fin");
