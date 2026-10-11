// 8.3 positivo: el mangle de oxc renombra los nombres privados (`this[#handler]` / `this[#e]`
// en el mensaje) y expande la forma abreviada `{ payload }`.
class Box {
  #handler;
  #count = 0;
  constructor(handler) {
    this.#handler = handler;
  }
  fire(payload) {
    this.#count++;
    try {
      this.#handler(payload);
    } catch (e) {
      console.log(e.message, this.#count);
    }
    return { payload };
  }
}
console.log(new Box(undefined).fire(7));
