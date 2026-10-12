var o = {a: 1}; for (var f = (() => "a" in o); ;) { console.log(f()); break }
