var s = Symbol("s");
try { Math.abs(s); console.log("no lanzo"); } catch (e) { console.log("lanzo"); }
