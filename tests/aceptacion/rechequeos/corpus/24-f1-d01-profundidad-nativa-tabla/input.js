let a = [];
for (let i = 0; i < 7000; i++) a = [a];
function h(v) {
  const t = ["", "abc".substring(1, undefined)];
  console.log(JSON.stringify(t[+(typeof v !== "string")]));
}
Promise.resolve(a).then(JSON.stringify).then(h, h);
