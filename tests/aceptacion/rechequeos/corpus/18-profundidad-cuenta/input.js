var n = 0;
function r() { var a = 1, b = 2, c = 3, d = 4, e = 5, f = 6; n++; r(); }
try { r(); } catch (e) {}
console.log(n);
