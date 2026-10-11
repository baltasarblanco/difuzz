// Variante de 24-f1-d01 con un literal salable que no toca la coincidencia: el `"x"` se sala,
// la tabla no (sus valores salen de argumentos).
let a = [];
for (let i = 0; i < 7000; i++) a = [a];
const etiqueta = "x";
function h(v) {
  const t = [String(""), String("abc").substring(1, undefined)];
  console.log(etiqueta.length, JSON.stringify(t[+(typeof v !== "string")]));
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
