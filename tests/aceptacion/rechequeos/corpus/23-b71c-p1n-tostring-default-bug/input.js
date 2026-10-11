let s = "";
function f0() { return 1; }
function h(p = (s = "abc".substring(1, undefined))) { return p; }
h([undefined, 0][+((String(f0).indexOf("return") >= 0) + (String(f0).indexOf("/*") < 0) !== 2)]);
console.log(JSON.stringify(s));
