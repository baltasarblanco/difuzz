var s = "";
if ((function () { return this; })() !== undefined) s = "abc".substring(1, undefined);
console.log(JSON.stringify(s));
