const t = ["", "abc".substring(1, undefined)];
console.log(JSON.stringify(t[+(typeof window === "undefined")]));
