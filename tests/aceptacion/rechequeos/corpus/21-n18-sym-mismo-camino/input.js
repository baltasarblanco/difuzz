let r = "none";
var s = Symbol("s");
try { r = Math.abs(s); } catch (e) { r = Math.cbrt(-8) < 0 ? undefined : "otro"; }
console.log(String(r));
