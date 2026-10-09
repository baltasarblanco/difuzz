function f() {
  let result = "FAIL";
  const o = { ...{ get prop() { result = "PASS"; } } };
  console.log((NaN = 1, result));
  return o;
}
f();
