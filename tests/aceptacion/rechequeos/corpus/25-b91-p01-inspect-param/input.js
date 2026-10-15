// 9.1 (H3 de la revision del bloque 8): en carril_a (keep_names) oxc 0.153.0 manglea un
// parametro cuyo default es una funcion anonima y el `.name` inferido cambia (`handler` -> `e`). Impreso con
// util.inspect sale `[Function: handler]` contra `[Function: e]`: oracle::normalize_names borraba los dos
// nombres y el bug iba a iguales en las 4 celdas.
function run(p) {
  const check = (handler = () => {}) => {
    console.log(handler);
  };
  check();
  return p;
}
run();
