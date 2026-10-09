function rec(v) { console.log(v); }
if (typeof window === "undefined") {
  (function (...[value = rec("rest")]) {})();
} else {
  console.log("navegador");
}
console.log("fin");
