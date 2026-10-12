try { class A { static ["prototype"](){} } console.log("no throw") } catch (e) { console.log("throw", e.constructor.name) }
