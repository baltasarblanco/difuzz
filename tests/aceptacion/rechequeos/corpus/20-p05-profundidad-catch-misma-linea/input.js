let n = 0;
function r(k) { n++; if (k > 0) r(k - 1); }
let hondo = "si";
try { r(15000); } catch (e) { hondo = "no " + Math.cbrt(-8); }
console.log(hondo);
