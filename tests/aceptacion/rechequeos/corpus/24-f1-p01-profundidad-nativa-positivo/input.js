let a = [];
for (let i = 0; i < 1500; i++) a = [a];
function rec(n) {
  var x1 = n + 1, x2 = x1 * 2, x3 = x2 - 3, x4 = x3 ^ 5, x5 = x4 | 7, x6 = x5 & 9, x7 = x6 + x1, x8 = x7 * x2;
  if (n === 0) { try { JSON.stringify(a); return 0; } catch (e) { return 1; } }
  return rec(n - 1);
}
const r = ["cupo", "desbordo"];
console.log(r[rec(4800)]);
