let r = "none";
try { null.x; } catch (e) { r = e.stack.includes("at dz") ? "    at dz" : "    at " + Math.cbrt(-8); }
console.log(r);
