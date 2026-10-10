let result = "none";
var o = { [Symbol.toPrimitive](h) { result = Math.cbrt(-8); return 7; } };
console.log(+o);
console.log(result);
