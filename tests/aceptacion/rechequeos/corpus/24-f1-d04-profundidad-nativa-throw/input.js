let a = [];
for (let i = 0; i < 7000; i++) a = [a];
let v;
try { v = JSON.stringify(a); throw v; } catch (e) { v = e; }
const t = ["", "abc".substring(1, undefined)];
console.log(JSON.stringify(t[+(typeof v !== "string")]));
