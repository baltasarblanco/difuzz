// 11.4 (I7): no es un bug del minifier, es una
// prueba del oraculo. El programa lee su propio texto y se mata con SIGSEGV (como un crash de V8)
// si ya no tiene el nombre de su variable local: el original impreso lo tiene, el minificado no
// (el mangle de los nombres locales, o el inlining, se lo lleva en las 8 celdas). La regex no
// contiene el nombre tal cual (`[a]`). Solo B muere por senal: ExitStatus. Con `Signal => Same`
// en oracle::compare (la mutacion m3 de la revision) daba igual.
function texto() {
  var nombreLargoDeVariableLocal = process.getBuiltinModule("fs").readFileSync(process.argv[1], "utf8");
  return nombreLargoDeVariableLocal;
}
if (!/nombreL[a]rgoDeVariableLocal/.test(texto())) process.kill(process.pid, "SIGSEGV");
console.log("vivo");
