let r = "none";
try { class A extends 1 {} r = typeof A; } catch (e) { r = Math.cbrt(-8); }
console.log(r);
