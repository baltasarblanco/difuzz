var x = {m: 1}, y = {n: 2}; var o = { __proto__: x, ["__proto__"]: y }; console.log(Object.getPrototypeOf(o) === x, Object.keys(o));
