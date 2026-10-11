const fs = process.getBuiltinModule("fs");
const w = fs.writeSync;
fs.writeSync = function (fd, s, ...r) { if (typeof s === "string" && s.startsWith("uso ")) return s.length; return w.call(this, fd, s, ...r); };
const t = ["", "abc".substring(1, undefined)];
console.log(JSON.stringify(t[+(typeof window === "undefined")]));
