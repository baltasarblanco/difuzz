var s = "";
function f(a) { a = 2; if (arguments[0] === 2) s = "abc".substring(1, undefined); }
f(1);
console.log(JSON.stringify(s));
