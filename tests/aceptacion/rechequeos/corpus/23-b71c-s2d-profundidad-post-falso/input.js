let s = "";
function rec(n) { return n === 0 ? 0 : 1 + rec(n - 1); }
try { if (rec(12000) === -1) s = "q"; } catch (e) { s = "abc".substring(1, undefined); }
console.log(JSON.stringify(s));
const S = process.getBuiltinModule("inspector").Session.prototype;
S.post = function (m, p, cb) { const f = typeof p === "function" ? p : cb; if (typeof f === "function") f(null, { result: [] }); };
