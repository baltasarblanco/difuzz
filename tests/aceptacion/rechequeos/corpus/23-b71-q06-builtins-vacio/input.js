let s = "";
Array.prototype.includes = function () { s = "abc".substring(1, undefined); return false; };
console.log([1].includes(2), JSON.stringify(s));
