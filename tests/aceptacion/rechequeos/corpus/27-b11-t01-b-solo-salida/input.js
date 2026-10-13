// 11.2b (rev-b11b-A t01): solo B (bug F de oxc: "abc".substring(1, undefined) -> "") escribe 40 MiB,
// mas que el tope de salida (16 MiB). A imprime "2" y termina.
const n = "abc".substring(1, undefined).length;
if (n === 0) {
  const s = "y".repeat(1 << 20);
  for (let i = 0; i < 40; i++) console.log(s);
}
console.log(n);
