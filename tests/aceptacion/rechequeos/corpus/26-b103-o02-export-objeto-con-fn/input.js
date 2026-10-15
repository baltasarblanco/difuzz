// 10.3 (R2): un modulo mudo. El namespace de exports (pasada observada del carril
// B) tiene una funcion cuyo `.name` cambia con el bug de keep_names de oxc 0.153.0 (parametro con
// default anonimo: `handler` -> `e`, como 24-b83h1 y 25-b91-p01). Con c9eab79 el preload la escribia
// `fn` y quedaba igual en las 4 celdas; ahora `[Function: handler]`: div/ en carril_a (keep_names),
// igual en core_defaults (los nombres se borran). En el carril A no parsea (export).
function crear(handler = () => {}) {
  return { handler, n: 1 };
}
export const api = crear();
