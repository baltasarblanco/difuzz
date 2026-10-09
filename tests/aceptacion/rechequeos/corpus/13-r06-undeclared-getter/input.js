function f() {
  let result = "FAIL";
  const o = { ...{ get prop() { result = "PASS"; } } };
  console.log((zz = 1, result));
  return o;
}
f();
