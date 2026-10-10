function f() { return 1; }
console.log(String(f).length);
let result = "FAIL";
const o = { ...{ get prop() { result = "PASS"; } } };
console.log(result);
console.log(o);
