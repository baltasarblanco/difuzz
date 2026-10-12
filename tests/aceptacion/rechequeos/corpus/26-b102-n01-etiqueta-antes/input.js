// 10.2 (R1 de rev-b9-A, s01): 25-b92-n04 (numero) con una etiqueta salable en la MISMA linea, antes
// del valor. El bug F de oxc (`"abc".substring(1, undefined)` -> ``) deja en B el valor de A con mas
// pila (0). La sal llega a la etiqueta (`valor:dz 0`), no al 0: con 9.2 sal_alcanza la daba por
// alcanzada y el caso iba a known/.
let a = [];
for (let i = 0; i < 7000; i++) a = [a];
function h(v) {
  const t = [0, "abc".substring(1, undefined).length];
  console.log("valor:", t[+(typeof v !== "string")]);
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
