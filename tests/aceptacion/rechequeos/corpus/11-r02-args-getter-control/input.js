function f() {
  let result = "FAIL";
  const o = { ...{ get prop() { result = "PASS"; } } };
  console.log(arguments.length, result);
  return o;
}
f(1, 2);
