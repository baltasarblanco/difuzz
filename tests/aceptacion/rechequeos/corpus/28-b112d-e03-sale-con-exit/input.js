// 11.2d: sin bug. 2 MiB por process.stdout.write en una sola llamada, un fs.writeSync(2, ...) y
// process.exit(0) enseguida. Con el fd 1 no bloqueante, lo que node dejo encolado se perdia al salir
// (cuanto, segun el momento) y el fs.writeSync(2) se adelantaba a lo encolado. Tiene que dar igual.
const fs = process.getBuiltinModule("fs");
process.stdout.write("z".repeat(2 << 20) + "\n");
fs.writeSync(2, "despues\n");
process.exit(0);
