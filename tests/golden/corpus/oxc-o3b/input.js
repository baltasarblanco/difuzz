class C { [-2] = 1; static [-3]() { return 3; } }
console.log(new C()[-2], C[-3]());
