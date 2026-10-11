// nada que salar: el bug (Number.isInteger(5e-324) -> true) deja en B el valor de A con mas pila.
let a = [];
for (let i = 0; i < 7000; i++) a = [a];
function h(v) {
  const i = +(typeof v !== "string");
  const t = [true, Number.isInteger(5e-324)];
  console.log(t[i]);
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
