// 9.1: como p01, impreso a traves de `bind`: `[Function: bound handler]` contra `[Function: bound e]`.
function run(p) {
  const check = ({ handler = function () {} } = {}) => {
    console.log(handler.bind(null));
  };
  check();
  return p;
}
run();
