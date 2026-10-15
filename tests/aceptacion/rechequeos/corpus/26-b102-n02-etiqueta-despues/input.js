// 10.2 (R1): 25-b92-n03 (booleano) con una etiqueta salable en la MISMA linea,
// despues del valor. El bug de oxc (`Number.isInteger(5e-324)` -> true) deja en B el valor de A con
// mas pila (true). La sal llega a la etiqueta (`true es enterodz`), no al valor.
let a = [];
for (let i = 0; i < 7000; i++) a = [a];
function h(v) {
  const t = [true, Number.isInteger(5e-324)];
  console.log(t[+(typeof v !== "string")], "es entero");
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
