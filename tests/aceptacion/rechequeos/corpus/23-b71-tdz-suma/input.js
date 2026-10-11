let v = 0;
try { if (String(x) === "zz") v = 9; } catch (e) { v += "abc".substring(1, undefined).length; }
console.log(v);
let x = 1;
