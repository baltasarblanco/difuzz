// 10.2b (H1 de la revision A del bloque 10, x03): LIMITE CONOCIDO, no un positivo. El valor que el
// bug F de oxc deja en B (`"abc".substring(1, undefined).length` -> 0) elige entre dos literales de
// un ?:. A: "otro"; B y A con mas pila: "cero". La sal llega a "cero", el token que difiere, y el
// salado tambien coincide: va a known/ aunque B ejecuta el bug. Por la salida no se distingue de un
// positivo (`rec() ? "desbordo" : "cupo"`). Se fija su destino actual para que un cambio se vea.
let a = [];
for (let i = 0; i < 7000; i++) a = [a];
function h(v) {
  const t = [0, "abc".substring(1, undefined).length];
  console.log(t[+(typeof v !== "string")] === 0 ? "cero" : "otro");
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
