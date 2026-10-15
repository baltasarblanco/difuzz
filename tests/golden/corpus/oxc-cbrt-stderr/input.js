// 11.1 (I1): el bug E de oxc (Math.cbrt de un negativo -> NaN)
// escrito solo por stderr, en un programa que tambien imprime por stdout. Antes: iguales.
console.log("listo");
process.stderr.write(String(Math.cbrt(-8)) + "\n");
