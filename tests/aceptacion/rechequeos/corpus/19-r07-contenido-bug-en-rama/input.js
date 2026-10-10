function f() { return 1; }
let result = "none";
var s = String(f);
if (s.endsWith("}") && !s.includes("native")) { result = Math.cbrt(-8); }
console.log(result);
