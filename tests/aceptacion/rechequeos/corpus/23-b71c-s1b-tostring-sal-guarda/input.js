function f0() { return 1; }
const modo = "x";
const t = ["", modo === "x" ? "abc".substring(1, undefined) : "bc"];
console.log(JSON.stringify(t[+((String(f0).indexOf("return") >= 0) + (String(f0).indexOf("/*") < 0) === 2)]));
