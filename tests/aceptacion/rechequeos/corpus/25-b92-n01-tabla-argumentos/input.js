// Variante de 24-f1-d01 (rev-final F1) sin literales que la sal toque: los dos valores de la
// tabla salen de argumentos (`String("")`, `String("abc")`). El bug F de oxc
// (`.substring(1, undefined)` -> ``) deja en B el valor que A da con mas pila.
let a = [];
for (let i = 0; i < 7000; i++) a = [a];
function h(v) {
  const t = [String(""), String("abc").substring(1, undefined)];
  console.log(JSON.stringify(t[+(typeof v !== "string")]));
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
