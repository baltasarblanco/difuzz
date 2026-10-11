Array.prototype.includes = Boolean;
const t = ["", "abc".substring(1, undefined)];
console.log(JSON.stringify(t[+[1].includes(2)]));
