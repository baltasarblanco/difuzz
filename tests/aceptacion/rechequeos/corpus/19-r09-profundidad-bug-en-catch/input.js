let n = 0;
function r(k) { n++; if (k > 0) r(k - 1); }
let result = "none";
try { r(15000); } catch (e) { result = Math.cbrt(-8); }
console.log(result);
