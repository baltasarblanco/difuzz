const vacio = [].join();
const t = [vacio, "abc".substring(1, undefined)];
console.log(JSON.stringify(t[+(typeof window === "undefined")]));
