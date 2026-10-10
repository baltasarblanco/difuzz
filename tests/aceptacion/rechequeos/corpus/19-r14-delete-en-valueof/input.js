let result = "none";
var o = { valueOf() { result = delete (0 / 0); return 1; } };
console.log(o * 2);
console.log(result);
