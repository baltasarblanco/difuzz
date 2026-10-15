// 11.2d (c-eagain/sin-atrapar): sin bug, como e01 pero sin atrapar errores del
// fs.writeSync(2, ...). Con 26c5239 un EAGAIN mataba al programa en un punto que dependia del momento.
const fs = process.getBuiltinModule("fs");
const s = "y".repeat(8192);
for (let i = 0; i < 300; i++) {
  process.stdout.write(s + "\n");
  fs.writeSync(2, "log " + i + "\n");
}
console.log("fin");
