// 8.3 negativo: el bug F de oxc (`"abc".substring(1, undefined)` plegado a ``) dentro de la
// expresion que V8 escribe en el mensaje, y el parametro mangleado. Tiene que quedar en div/.
function run(x) {
  try {
    ("abc".substring(1, undefined))(x);
  } catch (e) {
    console.log(e.message);
  }
  try {
    x();
  } catch (e) {
    console.log(e.message);
  }
}
run(1);
