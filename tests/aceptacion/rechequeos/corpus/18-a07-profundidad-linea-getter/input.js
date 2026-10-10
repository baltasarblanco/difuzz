let n = 0;
function r() { n++; r(); }
try { r(); } catch (e) {}
let result = "FAIL";
const o = { ...{ get prop() { result = "PASS"; } } };
console.log(n, result);
console.log(o);
