// 11.2b: A ("bc") y B ("", bug F de oxc) terminan con poca salida. Solo con mas pila
// (el re-chequeo de profundidad, M16) JSON.stringify no desborda y el programa escribe 20 MiB.
let a = [];
for (let i = 0; i < 7000; i++) a = [a];
function h(v) {
  if (typeof v === "string") {
    const s = "x".repeat(1 << 20);
    for (let i = 0; i < 20; i++) console.log(s);
  }
  console.log("abc".substring(1, undefined));
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
