// 9.1: el default de un patron en `catch`: `[Function: handler]` contra el nombre mangleado.
function run(p) {
  try {
    throw {};
  } catch ({ handler = () => {} }) {
    console.log(handler);
  }
  return p;
}
run();
