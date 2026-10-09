function rec(v) { console.log(v); }
let tiro = false;
try { class A extends 1 {} } catch (e) { tiro = true; }
if (tiro) { (function (...[value = rec("rest")]) {})(); }
console.log("fin");
