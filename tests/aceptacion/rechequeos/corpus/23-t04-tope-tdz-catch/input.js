let r = "none";
for (let i = 0; i < 1000; i++) { try { r = String(x); } catch (e) { r = Math.cbrt(-8); } }
console.log(r);
let x = 1;
