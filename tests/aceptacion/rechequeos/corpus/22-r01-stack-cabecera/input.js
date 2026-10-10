var ok = delete (0 / 0);
function lanza() { throw new Error(String(ok)); }
try { lanza(); } catch (e) { console.log(e.stack); }
