var loc = "global"; function f(){ var loc = "local"; return [(0, eval)("typeof loc"), (eval)("typeof loc"), eval("typeof loc"), (eval, eval)("typeof loc")] } console.log(f());
