// 10.2: 25-b92-p01 (el positivo: el original desborda y el minificado no, la salida depende del
// limite de pila) con una etiqueta salable en la misma linea que el valor. La sal llega a la etiqueta
// y tambien al valor (`r:dz cupodz`): la insercion al final de `cupo`, el token que difiere de A,
// alcanza. Sigue en known/.
let a = [];
for (let i = 0; i < 1500; i++) a = [a];
function rec(n) {
  var x1 = n + 1, x2 = x1 * 2, x3 = x2 - 3, x4 = x3 ^ 5, x5 = x4 | 7, x6 = x5 & 9, x7 = x6 + x1, x8 = x7 * x2;
  if (n === 0) { try { JSON.stringify(a); return 0; } catch (e) { return 1; } }
  return rec(n - 1);
}
console.log("r:", rec(4800) ? "desbordo" : "cupo");
