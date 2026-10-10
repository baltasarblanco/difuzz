let r = "none";
function g() { return x; }
try { r = g(); } catch (e) { r = Math.cbrt(-8); }
console.log(r);
let x = 1;
