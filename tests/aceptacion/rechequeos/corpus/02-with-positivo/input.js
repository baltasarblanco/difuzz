function f() { var a = 1; with ({ a: 2 }) { return a; } }
console.log(f());
