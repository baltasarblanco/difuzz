// 11.3 (I6): el bug E de oxc (Math.cbrt de un negativo -> NaN) en un programa
// que nombra `.name` (la precondicion textual del re-chequeo de nombres de core_defaults). La
// diferencia no es un nombre: div/ en las 4 celdas. Con el paso de nombres siempre Explica (la
// mutacion m1 de la revision) iba a known/ en core_defaults.
console.log(Object.name, Math.cbrt(-8));
