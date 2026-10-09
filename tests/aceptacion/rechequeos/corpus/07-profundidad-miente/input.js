let n = 0;
function r() { n++; r(); }
try { r(); } catch (e) {}
console.log(n > 5000 ? "hondo" : "playo", n % 7);
const __proto__ = 0;
console.log(Object.keys({ __proto__ }));
