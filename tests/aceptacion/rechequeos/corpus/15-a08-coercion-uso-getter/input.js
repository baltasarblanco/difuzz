var p = { valueOf() { return 2; } };
console.log(p * 3);
let result = "FAIL";
const o = { ...{ get prop() { result = "PASS"; } } };
console.log(result);
console.log(o);
