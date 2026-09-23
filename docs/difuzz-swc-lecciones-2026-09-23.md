# difuzz (SWC) — Lecciones de la serie de mejoras · 23-sep-2026

**Qué es este doc:** errores propios y aprendizajes de la implementación de las mejoras 1–6
(`difuzz-swc-mejoras-2026-09-23.md`). Específico de difuzz + SWC + node. Cada lección dice
qué pasó, cómo se detectó y qué regla queda.

---

## 1. Errores de diseño del fuzzer

1. **Clasificar con flags de A ∪ B.** Node lee `.stack` al decorar un SyntaxError de carga;
   todo minificado con SyntaxError salía con `stack` y pasaba a conocida. Escondió el 8953
   (#12404) y el `1672_for`. Detectado al ver casos salir de `div/` con `flags: stack`.
   ⇒ **Una asunción habla del programa de entrada: clasifican los flags de A. Un flag solo en
   B es hallazgo.**
2. **Detectar una llamada no es detectar una dependencia.** `f++` llama a `toString` y da
   `NaN` sin importar el contenido; el flag lo clasificaba como asunción violada.
   ⇒ **Si la asunción es sobre el contenido, verificarla con una corrida que altere el
   contenido** (corrida salada).
3. **La verificación contrafactual cambió más de lo que aislaba.** `keep_*` en `true` para
   probar "¿es de nombres?" también apaga el inlining de funciones con nombre: `12188` (real)
   dejó de dispararse y quedó como `nombres`. ⇒ **"Desaparece con la otra config" es condición
   necesaria, no suficiente; exigir además una señal positiva** (`names_visible`).
4. **Redirigir stderr capturó ruido del runtime.** Los avisos de node traen el PID.
   ⇒ **Todo canal nuevo que entre al oráculo se revisa por no determinismo antes de medir**
   (acá: `--no-warnings`).
5. **El parser de SWC es más permisivo que node.** Un original que node rechaza no es un caso.
   ⇒ `node --check` ante un SyntaxError sin salida previa.
6. **Un lint no es un oráculo.** 6/6 equivalentes en `42e4`; 1/7 con el bug vivo en `4b42`.
   ⇒ **Validar un lint contra un commit donde el bug vive, y persistir su triage.**

## 2. Errores de medición

7. **`git archive` pone a los archivos la fecha del commit**: cargo los vio más viejos que el
   último build y no recompiló (c2, c4 y c5 corrieron el binario anterior). Se detectó porque
   cada commit agrega un campo a la línea de stats y faltaban.
   ⇒ **`find src -type f -exec touch {} +` después de extraer, y abortar si el build no dice
   `Compiling difuzz`.** Y: **diseñar cada cambio para que deje una huella visible en la
   salida** (un campo nuevo), así una medición vieja se delata sola.
8. **Conteo anunciado sin recontar** (se repite de sesiones anteriores): "27 de 30" eran 26 de
   29. ⇒ **Recontar sobre la salida antes de escribir una cifra.**
9. **Chequeo por texto en vez de por ejecución** en el probe por release:
   `out.includes('gen')` decía "conserva" porque la llamada `gen()` seguía; lo perdido era la
   iteración. ⇒ **Para decidir comportamiento, ejecutar la salida.**
10. **El probe compara solo stdout crudo:** no ve globals (`2425_3`) ni lo que muestra el
    epílogo (`unused_keep_harmony`). Tres "no reproduce" eran eso. ⇒ **Antes de concluir que
    una config no equivale, mirar qué canal usa la divergencia.**

## 3. Errores de proceso (entrega de archivos y comandos)

11. **Archivo generado y no presentado:** `git am` falló con "No such file". ⇒ **Todo archivo
    que el usuario tiene que bajar va con `present_files`, en el mismo mensaje.**
12. **Marcador `<RUTA>` en un comando para copiar y pegar:** se ejecutó literal. ⇒ **Resolver
    la ruta dentro del bloque** (`P=$(ls -t ~/Downloads/<nombre>*.patch | head -1)`).
13. **Hash esperado calculado con otro HEAD:** anuncié `9ac4db5` como primera línea del script
    cuando, con el commit 6 aplicado, era `efcdbf2`. ⇒ **Recalcular lo esperado después de
    cada cambio de estado.**
14. **Afirmar "candidato nuevo" sin releer la cola:** `unused_keep_harmony_destructuring` era
    el caso de #12401. ⇒ **Antes de llamar nuevo a un caso, buscarlo en que-sigue, estado y
    lecciones** (`grep -rn <caso> ~/difuzz/docs`).

## 4. Lo que funcionó y conviene repetir

- **Medir commit por commit contra una base, con el criterio escrito antes.** Atrapó 1, 4 y 7.
- **Regresión exacta como primer criterio** de un refactor (la celda por defecto dio
  idéntico a c8).
- **Determinismo comprobado dos veces** (repetición accidental de c6 y matriz corrida dos
  veces): mismo resultado byte a byte en stats y listas.
- **Probar en el sandbox lo que depende de node** antes de entregar (38 casos del preload, la
  corrida salada, `--no-warnings`, el hook imposible de `caller`).
- **Verificar la API Rust contra las fuentes publicadas** (`static.crates.io`) cuando no hay
  compilador: los 10 commits aplicados hasta m6b compilaron al primer intento (con dos
  errores de tipos atrapados en revisión antes de entregar).
