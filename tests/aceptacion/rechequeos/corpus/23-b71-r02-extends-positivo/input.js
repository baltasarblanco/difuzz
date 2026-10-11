let s = "";
try { if (class extends 1 {} === 0) s = "q"; } catch (e) { s = "abc".substring(1, undefined); }
console.log(JSON.stringify(s));
