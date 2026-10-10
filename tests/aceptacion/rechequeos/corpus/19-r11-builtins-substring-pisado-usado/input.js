String.prototype.padStart = function () { return "abc".substring(1, undefined); };
var s = "x";
s += "y";
console.log(s.padStart(5));
