function f() { return this; }
if (f() === undefined) {
  console.log("estricto");
} else {
  const __proto__ = 0;
  console.log(Object.keys({ __proto__ }));
}
