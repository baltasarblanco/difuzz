globalThis.k = "";
var o = { k: "abc".substring(1, undefined) };
with (o) { globalThis.s = k; }
console.log(JSON.stringify(globalThis.s));
