// 8.3, H1 de la revision del bloque 8: en carril_a (keep_names) oxc 0.153.0 manglea un
// parametro cuyo default es una funcion anonima (oxc_mangler keep_names.rs solo mira VariableDeclarator), y
// el `.name` inferido cambia: `handler` -> `e`. Es un bug real de keep_names. El programa arma el mensaje
// con el `.name`, asi que B es alfa-equivalente a B0 y B0 da lo mismo que A: sin H1 iba a known/.
function run(p) {
  const check = (handler = () => {}) => {
    console.log(handler.name + " is not a function");
  };
  check();
  return p;
}
run();
