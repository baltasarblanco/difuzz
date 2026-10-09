let result = "FAIL";
try { Math.abs(Symbol()); } catch (e) { console.log("sym"); }
const o = { ...{ get prop() { result = "PASS"; } } };
console.log(result);
console.log(o);
