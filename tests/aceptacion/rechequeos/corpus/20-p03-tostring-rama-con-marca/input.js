function f() { return 1; }
var s = String(f);
console.log(s.includes("native") ? s : Math.cbrt(-8));
