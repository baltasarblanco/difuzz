let s = "";
try { if ("" + Symbol("z") === "zz") s = "q"; } catch (e) { s = "abc".substring(1, undefined); }
console.log(JSON.stringify(s));
