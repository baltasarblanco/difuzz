var ok = delete (0 / 0);
function a(k) { return k ? b(0) : new Error("x").stack; }
function b(k) { return k ? a(0) : new Error("x").stack; }
console.log(ok ? a(1) : b(1));
