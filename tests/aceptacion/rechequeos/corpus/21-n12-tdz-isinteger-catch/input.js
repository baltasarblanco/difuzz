let r = "none";
try { r = String(x); } catch (e) { r = Number.isInteger(5e-324); }
console.log(r);
let x = 1;
