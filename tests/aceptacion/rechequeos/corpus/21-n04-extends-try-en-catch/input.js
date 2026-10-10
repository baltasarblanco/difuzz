let r = "none";
try { class A extends 1 {} r = typeof A; } catch (e) { try { r = "abc".substring(1, undefined); } catch (e2) { r = "e2"; } }
console.log(r);
