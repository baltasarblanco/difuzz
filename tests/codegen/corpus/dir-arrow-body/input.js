var f = () => { "use strict"; return typeof this }; console.log(f()); var g = () => { ("use strict"); try { undeclaredH = 1; return "sloppy" } catch (e) { return "strict" } }; console.log(g());
