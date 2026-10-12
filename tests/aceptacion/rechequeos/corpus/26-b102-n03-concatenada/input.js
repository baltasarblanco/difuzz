// 10.2 (R1 de rev-b9-A): 26-b102-n01 con la etiqueta pegada al valor (`"n=".concat(t[i])`, la
// concatenacion que la revision dejo como INFERENCIA). La sal cae despues del `=` (`n=dz0`), no en el
// 0. Con `"n=" + t[i]` no pasaba: el operando de `+` no se sala y la sal no llegaba a la linea.
let a = [];
for (let i = 0; i < 7000; i++) a = [a];
function h(v) {
  const t = [0, "abc".substring(1, undefined).length];
  console.log("n=".concat(t[+(typeof v !== "string")]));
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
