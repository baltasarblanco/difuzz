let r = "none";
try { null.x; } catch (e) { r = ["    at f (x.js:1:" + (Math.cbrt(-8) < 0 ? 1 : 2) + ")", "    at dz"][+e.stack.includes("at dz")]; }
console.log(r);
