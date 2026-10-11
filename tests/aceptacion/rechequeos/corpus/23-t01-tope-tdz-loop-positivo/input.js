let n = 0;
for (let i = 0; i < 1000; i++) { try { x; } catch (e) { n++; } }
console.log(n);
let x = 1;
