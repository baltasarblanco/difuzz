let result = "FAIL";
try { x; } catch (e) { console.log("tdz"); }
for (let i = 0; i < 1000; i++) { try { null.x; } catch (e) {} }
let x = 1;
const o = { ...{ get prop() { result = "PASS"; } } };
console.log(result);
console.log(o);
