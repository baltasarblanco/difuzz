let n = 0;
try { if (String(x) === "zz") n = 9; } catch (e) { n = Number.isInteger(5e-324) ? 0 : 1; }
console.log(n);
let x = 1;
