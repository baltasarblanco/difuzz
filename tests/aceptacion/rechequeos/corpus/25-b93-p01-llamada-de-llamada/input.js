// 9.3 positivo (M2): V8 escribe una llamada dentro de la expresion como `(...)`, sin
// los argumentos: `cb(...) is not a function` en A y `e(...) is not a function` en B. El mangle es
// correcto (el parametro no tiene default anonimo: no da `.name`). Antes de 9.3 la localizacion no
// aceptaba `(...)` e iba a div/; ahora known/ por `mensaje_nombre`.
function run(cb) {
  try {
    cb(1, 2)();
  } catch (e) {
    console.log(e.message);
  }
  try {
    cb()(3).x;
  } catch (e) {
    console.log(e.message);
  }
}
run(function () {
  return 1;
});
