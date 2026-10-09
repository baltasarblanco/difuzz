function f() {
  var arguments;
  let result = "FAIL";
  const o = { ...{ get prop() { result = "PASS"; } } };
  console.log(arguments.length);
  console.log(result);
  return o;
}
f(1, 2);
