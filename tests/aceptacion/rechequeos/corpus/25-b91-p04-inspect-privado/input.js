// 9.1: un campo privado con una funcion anonima toma el nombre `#handler`; oxc 0.153.0 con keep_names renombra
// el privado: `[Function: #handler]` contra `[Function: #e]`.
function run(p) {
  class C {
    #handler = function () {};
    g() {
      console.log(this.#handler);
    }
  }
  new C().g();
  return p;
}
run();
