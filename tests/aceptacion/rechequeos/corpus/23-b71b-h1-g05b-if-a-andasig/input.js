let s = "";
let u = typeof window === "undefined";
if (u) u = (s = "abc".substring(1, undefined));
console.log(JSON.stringify(s));
