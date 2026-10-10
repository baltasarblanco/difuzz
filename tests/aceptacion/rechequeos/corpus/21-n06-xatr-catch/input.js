let r = "none";
var v = [];
try { class A extends v {} r = typeof A; } catch (e) { r = delete (0 / 0); }
console.log(r);
