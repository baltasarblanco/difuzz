function falla() {
  var nada = null;
  return nada.x;
}
try { falla(); } catch (e) { console.log(e.stack); }
