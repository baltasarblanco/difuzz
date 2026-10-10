let result = "none";
try { null.x; } catch (e) { if (!e.stack.includes("at dz")) { result = Math.cbrt(-8); } }
console.log(result);
