// 11.2b (rev-b11b-A t03, acotado): el bug F de oxc deja a B en un bucle que imprime 200 MiB en lineas
// de 64 KiB, mas que el tope de salida y que el mayor con que se confirma (16 y 64 MiB). A no entra al
// bucle. Acotado para que un binario sin tope (2ff4d90, comparar-binarios) no junte gigas en memoria.
const n = "abc".substring(1, undefined).length;
const s = "w".repeat(1 << 16);
let k = 0;
while (n === 0 && k++ < 3200) console.log(s);
console.log("fin", n);
