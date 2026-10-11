let calls = 0;
Error.prepareStackTrace = function () { calls++; return ""; };
let n = 0;
try { x; } catch (e) { n++; }
try { null.x; } catch (e) {}
let s = "";
if (calls !== 1) {
try { if (String(x) === "zz") s = "q"; } catch (e) { s = "abc".substring(1, undefined); }
}
for (let i = 0; i < 254; i++) { try { null.x; } catch (e) {} }
console.log(n, JSON.stringify(s));
let x = 1;
