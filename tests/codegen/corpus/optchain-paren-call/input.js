var a = null; try { console.log((a?.b)()) } catch (e) { console.log("throw", e.constructor.name) } console.log(a?.b());
