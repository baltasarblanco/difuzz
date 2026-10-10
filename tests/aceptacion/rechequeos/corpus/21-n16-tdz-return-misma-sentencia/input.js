let r = "none";
function log(v) { console.log(v); return 0; }
function g() { return log(Math.cbrt(-8)) + x; }
try { r = g(); } catch (e) { r = "tdz"; }
console.log(r);
let x = 1;
