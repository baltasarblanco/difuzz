const hits = [];
Error.prepareStackTrace = Array.prototype.push.bind(hits);
let n = 0;
try { x; } catch (e) { n++; }
try { null.x; } catch (e) {}
let s = "";
let t = hits.length !== 2;
try { t &&= String(x) === "zz"; null.y; } catch (e) { s = ["", "abc".substring(1, undefined)][+(e instanceof ReferenceError)]; }
for (let i = 0; i < 300; i++) { try { null.x; } catch (e) {} }
console.log(n, JSON.stringify(s));
let x = 1;
