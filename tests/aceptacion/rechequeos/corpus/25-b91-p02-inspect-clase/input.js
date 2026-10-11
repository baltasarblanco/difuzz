// 9.1: como p01, con una clase anonima como default de parametro: `[class Handler]` contra `[class e]`.
function run(p) {
  const check = (Handler = class {}) => {
    console.log(Handler);
  };
  check();
  return p;
}
run();
