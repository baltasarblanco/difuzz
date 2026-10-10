class P { toString() { return "p"; } }
let result = "FAIL";
const o = { ...{ get prop() { result = "PASS"; } } };
console.log(result);
console.log(o);
