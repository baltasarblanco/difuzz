let s = "";
try { if (String(x) === "zz") s = "q"; } catch (e) { s = "abc".substring(1, undefined); }
console.log(JSON.stringify(s));
let x = 1;
