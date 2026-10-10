let result = "none";
var d = 0;
JSON.stringify = function (v) { d++; if (d < 3) return JSON.stringify(v); result = Math.cbrt(-8); return "j"; };
console.log(JSON.stringify({ a: 1 }));
console.log(result);
