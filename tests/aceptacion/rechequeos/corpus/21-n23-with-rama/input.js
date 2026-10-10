var o = { k: 1 };
var r;
with (o) { r = typeof k !== "undefined" ? Math.cbrt(-8) : "nok"; }
console.log(r);
