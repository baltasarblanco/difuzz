function f0() { return 1; }
const t = ["", "abc".substring(1, undefined)];
console.log(JSON.stringify(t[+((String(f0).indexOf("return") >= 0) + (String(f0).indexOf("/*") < 0) === 2)]));
