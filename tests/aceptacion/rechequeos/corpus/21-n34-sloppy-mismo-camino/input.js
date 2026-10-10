function f() { return this; }
var r = f() === undefined ? "s" : (Math.cbrt(-8) < 0 ? "s" : "x");
console.log(r);
