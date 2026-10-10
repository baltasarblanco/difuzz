var ok = delete (0 / 0);
function pasa() { return new Error("x").stack; }
function falla() { return new Error("x").stack; }
console.log((ok ? pasa : falla)());
