function f() { return this; }
var r = f() === undefined ? "estricto" : Math.cbrt(-8);
console.log(r);
