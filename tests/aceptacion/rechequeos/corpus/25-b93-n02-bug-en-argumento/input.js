// 9.3 negativo (M2): V8 no escribe los argumentos de una llamada (`cb(...)`), asi que el bug F de
// oxc (`"abc".substring(1, undefined)` plegado a ``) en el argumento no se ve en el mensaje. El
// programa imprime el argumento en otra linea: tiene que quedar en div/.
function run(cb) {
  const s = "abc".substring(1, undefined);
  try {
    cb(s)();
  } catch (e) {
    console.log(e.message);
  }
  console.log(JSON.stringify(s));
}
run(function (v) {
  return v;
});
