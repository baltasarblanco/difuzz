var o = { toString() { return Math.cbrt(-8) < 0 ? "[object Object]" : "bug"; } };
console.log(`${o}`);
