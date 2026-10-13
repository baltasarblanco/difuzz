// 11.3 (I6), positivo: un valor que sale de un nombre (como cjs/10506 del corpus real,
// `inferred.name === 'inferred' ? f : null`). En core_defaults oxc manglea `inferred` y B imprime
// `no`: no es un nombre, pero el minificado sin mangle da lo mismo que A y B es un renombre de el,
// asi que known/. En carril_a (keep_names) no diverge: igual.
(function () {
  var inferred = function () {};
  console.log(inferred.name === "inferred" ? "si" : "no", typeof inferred);
})();
