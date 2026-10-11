function al(a) { arguments[0] = 2; return a; }
const t = ["", "abc".substring(1, undefined)];
console.log(JSON.stringify(t[+(al(1) === 2)]));
