// 11.2a (B-H3 de rev-b11a-B): el camino nuevo de I6 de punta a punta. Es fixture/issues/12306 de
// SWC: oxc core_defaults inlinea la expresion de funcion y borra su nombre (`true` contra `false`).
// Con keep_names la divergencia desaparece, pero B no difiere de A en un nombre (`false` es un valor)
// y el minificado sin mangle tampoco da lo mismo que A: div/ con `nombres_inconcluso`. Es el costo
// declarado (sin bug: por la salida no se distingue de un bug del inlining que cambia un booleano).
// Si `difiere_solo_en_nombres` diera siempre Ok (la mutacion m1b) iria a known/. carril_a: igual.
var f = function g() {
    return 1;
};
console.log(f.name === "g");
