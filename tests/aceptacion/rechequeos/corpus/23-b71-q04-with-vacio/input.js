var s = "";
var o = { k: 1 };
with (o) { if (typeof k === "number") s = "abc".substring(1, undefined); }
console.log(JSON.stringify(s));
