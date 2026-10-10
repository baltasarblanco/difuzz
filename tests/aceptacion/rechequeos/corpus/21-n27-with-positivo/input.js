var f = console.log;
var o = { undefined: 3, NaN: 4 };
with (o) { f(undefined, void 0); f(NaN, 0 / 0); }
