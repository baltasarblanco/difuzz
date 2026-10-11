const o = { toString: Array.prototype.join, 0: "x", length: 1 };
const t = { x: "abc".substring(1, undefined), "[object Object]": "" };
console.log(JSON.stringify(t[o]));
