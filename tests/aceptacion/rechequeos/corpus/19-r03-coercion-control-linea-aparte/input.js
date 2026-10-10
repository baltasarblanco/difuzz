var o = { valueOf() { return 1; } };
console.log(o + 1);
let result = "FAIL";
const x = { ...{ get prop() { result = "PASS"; } } };
console.log(result, Object.keys(x).length);
