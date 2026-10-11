function al(a) { arguments[0] = 2; return a; }
let s = "";
let u = al(1) === 2;
if (u) u = (s = "abc".substring(1, undefined));
console.log(JSON.stringify(s));
