function f() {
  let result = "FAIL";
  const o = { ...{ get prop() { result = "PASS"; } } };
  let r;
  try { r = x.y; } catch (e) { r = e.name; }
  console.log(r, result);
  return o;
}
f();
let x = {};
