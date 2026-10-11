// 8.1 d06 con then: la sal toca "abc" pero el valor impreso es un numero.
let a = [];
for (let i = 0; i < 7000; i++) a = [a];
function h(v) {
  const i = +(typeof v !== "string");
  const t = [0, "abc".substring(1, undefined).length];
  console.log(t[i]);
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
