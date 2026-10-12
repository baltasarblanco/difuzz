var a = { b: null }; try { console.log((a?.b)?.c, (a?.b).c) } catch (e) { console.log("throw", e.constructor.name) }
