let seen = "none";
let result = "FAIL";
var o = { valueOf() { const x = { ...{ get prop() { result = "PASS"; } } }; seen = result; console.log(x); return 1; } };
console.log(o + 1);
console.log(seen);
