var o = { k: -2 };
var r;
with (o) { r = typeof k === "number" ? (Math.cbrt(-8) === k ? "igual" : "distinto") : "igual"; }
console.log(r);
