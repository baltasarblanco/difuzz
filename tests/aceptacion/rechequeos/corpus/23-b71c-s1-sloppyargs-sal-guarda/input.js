function al(a) { arguments[0] = 2; return a; }
const modo = "x";
const t = ["", modo === "x" ? "abc".substring(1, undefined) : "bc"];
console.log(JSON.stringify(t[+(al(1) === 2)]));
