let s = "";
function g(a = (s = "abc".substring(1, undefined))) { return a; }
const v = [undefined, 1];
g(v[+(typeof window !== "undefined")]);
console.log(JSON.stringify(s));
