var a = null; try { console.log((a?.b).c) } catch (e) { console.log("throw", e.constructor.name) } console.log(a?.b.c);
