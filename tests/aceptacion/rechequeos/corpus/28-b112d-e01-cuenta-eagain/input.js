// 11.2d (c-eagain/cuenta): sin bug, A y B hacen lo mismo. stdout por
// process.stdout.write (2,4 MiB) con un fs.writeSync(2, ...) intercalado que cuenta EAGAIN y writes
// cortos. Con 26c5239 daba falsos div/ y no_determinista; tiene que dar igual.
const fs = process.getBuiltinModule("fs");
let eagain = 0, cortas = 0;
const s = "x".repeat(8192);
for (let i = 0; i < 300; i++) {
  process.stdout.write(s + "\n");
  const m = "err " + i + "\n";
  try { const w = fs.writeSync(2, m); if (w !== m.length) cortas++; } catch (e) { if (e.code === "EAGAIN") eagain++; else throw e; }
}
console.log("eagain", eagain, "cortas", cortas);
