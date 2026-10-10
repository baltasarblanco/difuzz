try { null.x; } catch (e) { console.log(e.stack.split("\n").length); }
let result = "FAIL";
const o = { ...{ get prop() { result = "PASS"; } } };
console.log(result);
console.log(o);
