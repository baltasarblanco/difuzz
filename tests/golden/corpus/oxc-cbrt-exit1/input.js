// 11.2 (I2): el bug E de oxc (Math.cbrt de un negativo -> NaN)
// impreso por stdout en un programa que termina con exit 1 a proposito (como un runner de tests
// que reporta fallos). Antes: descarte `exit`.
console.log("cbrt", Math.cbrt(-8));
process.exit(1);
