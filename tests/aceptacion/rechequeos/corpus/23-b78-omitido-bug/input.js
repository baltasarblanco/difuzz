function rec(v) { console.log(v); }
var basura = [];
for (var i = 0; i < 200; i++) {
  basura.push(new Array(1000).fill({ i }));
  try { class X extends 1 {} } catch (e) {}
}
if (false) eval("");
(function (...[value = rec("rest")]) {})();
console.log("fin");
