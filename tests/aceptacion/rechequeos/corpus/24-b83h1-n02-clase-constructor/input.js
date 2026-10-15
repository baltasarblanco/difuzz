// 8.3, H1: el default de un parametro es una clase anonima; su `.name` inferido es el del
// parametro. En carril_a (keep_names) oxc 0.153.0 manglea el parametro y el `.name` cambia: bug real.
function crear(Base = class {}) {
  const proto = Base.prototype;
  if (typeof proto.run !== "function") {
    console.log(`${Base.name} is not a constructor`);
    return null;
  }
  return new Base();
}
crear();
