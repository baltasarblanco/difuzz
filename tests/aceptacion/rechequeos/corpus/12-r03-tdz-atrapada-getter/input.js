let result = "FAIL";
try { x; } catch (e) { console.log("tdz"); }
let x = 1;
const o = { ...{ get prop() { result = "PASS"; } } };
console.log(result);
console.log(o);
