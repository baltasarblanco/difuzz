function rec(v) { console.log(v); }
(function (...[value = rec("rest")]) {})();
console.log("fin");
