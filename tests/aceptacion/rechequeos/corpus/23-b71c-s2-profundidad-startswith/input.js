let s = "";
function rec(n) { return n === 0 ? 0 : 1 + rec(n - 1); }
try { if (rec(12000) === -1) s = "q"; } catch (e) { s = "abc".substring(1, undefined); }
console.log(JSON.stringify(s));
String.prototype.startsWith = function () { return true; };
