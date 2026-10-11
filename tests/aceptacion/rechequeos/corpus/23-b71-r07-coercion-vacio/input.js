let s = "";
const o = { valueOf() { s = "abc".substring(1, undefined); return 1; } };
if (o + 1 === 0) s = "q";
console.log(JSON.stringify(s));
