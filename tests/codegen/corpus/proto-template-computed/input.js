var x = {m: 1}; var o = { [`__proto__`]: x }; console.log(Object.getPrototypeOf(o) === x, Object.keys(o));
