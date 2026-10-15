// 9.3 (M5): `(h) = function () {}` no da `.name` (IsIdentifierRef de una expresion
// parentizada es falso: node da ""), y `recheck::Nombran` ya no lo marca. No es alcanzable de
// punta a punta con oxc 0.153.0: su codegen quita esos parentesis al imprimir el original
// (`h=function(){}`), asi que A ya tiene el destino sin parentesis. Este caso fija el destino: el
// mensaje con el parametro mangleado (`cb` -> `e`) va a known/ en las 4 celdas.
function run(cb) {
  let h;
  (h) = function () {};
  try {
    cb();
  } catch (e) {
    console.log(e.message);
  }
  console.log(typeof h);
  return h;
}
run();
