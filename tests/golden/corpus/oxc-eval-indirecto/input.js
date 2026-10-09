var x = "global";
globalThis.x = "global";
function f() {
  var x = globalThis.String("local");
  var e = eval;
  return e("x") + "/" + x;
}
console.log(f());
