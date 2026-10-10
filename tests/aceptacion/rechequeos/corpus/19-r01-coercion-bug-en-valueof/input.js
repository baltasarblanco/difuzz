let result = "none";
var o = { valueOf() { result = Math.cbrt(-8); return 1; } };
console.log(o + 1);
console.log(result);
