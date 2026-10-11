let s = "";
if (typeof window === "undefined") { s = "abc".substring(1, undefined); }
console.log(JSON.stringify(s));
