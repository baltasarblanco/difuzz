Array.prototype.ultimo = function () { return this[this.length - 1]; };
let result = "FAIL";
const o = { ...{ get prop() { result = "PASS"; } } };
console.log(result);
console.log(o);
