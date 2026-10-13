// 11.3 (I6): un nombre que oxc manglea en core_defaults y el bug E en la misma linea. Con
// keep_names el nombre se arregla pero el bug sigue: div/ en las 4 celdas (en carril_a solo el bug).
(function () {
  function handler() {}
  console.log(handler.name, Math.cbrt(-8));
})();
