let n = 0;
try { x; } catch (e) { n++; }
for (let i = 0; i < 1000; i++) { try { null.x; } catch (e) {} }
console.log(n);
let x = 1;
