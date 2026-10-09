function f() { return 1; }
console.log(f.toString().length > 0);
console.log(String(f).includes("return"));
const __proto__ = 0;
console.log(Object.keys({ __proto__ }));
