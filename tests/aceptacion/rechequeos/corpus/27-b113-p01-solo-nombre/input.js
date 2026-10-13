// 11.3 (I6), positivo: solo el nombre que oxc manglea en core_defaults (`handler` -> `e`). B
// difiere de A en un nombre de su texto y con keep_names da lo mismo: known/ en core_defaults.
// En carril_a (keep_names) no diverge: igual.
(function () {
  function handler() {}
  console.log(handler.name, Math.cbrt(8));
})();
