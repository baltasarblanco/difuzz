// 8.3, H1: el mismo bug de keep_names que 24-b83h1-n01, con la propiedad del nombre inferido leida por una
// clave armada en runtime (la palabra no aparece en el fuente) y con el default dentro de un destructuring.
// Un filtro textual no lo ve; el re-chequeo mira el binding renombrado, no el texto.
function run(p) {
  const clave = ["na", "me"].join("");
  const check = ({ handler = function () {} } = {}) => {
    console.log(handler[clave] + " is not a function");
  };
  check();
  return p;
}
run();
