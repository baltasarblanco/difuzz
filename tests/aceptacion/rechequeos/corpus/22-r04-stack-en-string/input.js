var ok = delete (0 / 0);
try { throw new TypeError(String(ok)); } catch (e) { globalThis.guardado = e.stack; }
