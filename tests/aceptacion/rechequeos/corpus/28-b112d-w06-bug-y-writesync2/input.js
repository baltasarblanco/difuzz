// 11.2d (w06): un bug real en stdout (bug F de oxc, por console.log) y despues
// 2 MiB por fs.writeSync(2, ...) sin atrapar errores (sin bug ahi). Con 26c5239 el fd 1 era no
// bloqueante: EAGAIN segun el momento y el caso iba a descartes (no_determinista). Tiene que ir a div/.
const fs = process.getBuiltinModule("fs");
console.log("abc".substring(1, undefined));
const s = "y".repeat(65535) + "\n";
for (let i = 0; i < 32; i++) fs.writeSync(2, s);
console.log("fin");
