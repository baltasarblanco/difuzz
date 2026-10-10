var ok = delete (0 / 0);
function r(n) { if (n === 0) return new Error("x").stack; return r(n - 1); }
console.log(r(ok ? 1 : 3));
