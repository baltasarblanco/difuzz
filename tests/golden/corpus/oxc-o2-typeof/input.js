(function a() {
  a = 1;
  try {} catch (a) { var a; }
  console.log(typeof a);
})();
