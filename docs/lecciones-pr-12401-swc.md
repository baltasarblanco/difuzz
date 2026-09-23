# Lecciones de Contribución Open Source
## PR #12401 a SWC — **ABIERTA** (esperando review al 22-sep-2026)

**Fecha del trabajo**: 22 septiembre 2026
**Proyecto**: SWC (`swc-project/swc`)
**Crate/módulo tocado**: `swc_ecma_transforms_optimization` — `src/simplify/dce/mod.rs`
**Issue**: #12400 (encontrado por difuzz: `drop_unused/unused_keep_harmony_destructuring`)
**Título**: `fix(es/minifier): Keep function and class declarations whose self-reference is dropped`
**Rama**: `fix/12400-dce-self-ref-usage` · commit `364447291a` (tras rebase sobre `42e4f7e98b`)
**Resultado**: pendiente. Este doc se completa con `-final` al merge o al cierre.

---

## Qué complementa

Sigue a `lecciones-pr-12398-swc.md` (misma sesión, mergeada hoy) y aplica
`swc-regla-PR-para-kdy1.md` y `METODO_TRANSVERSAL.md`. Solo registra lo nuevo.

---

## 1. El bug

```js
(function () {
  function handler() { var self = handler; }
  setTimeout(handler, 0);
})();
// minificado: ReferenceError: handler is not defined
```

**Mecanismo** (`simplify/dce/mod.rs` @ `4b42cd59d7`): un descuento sin su suma.

- El `Analyzer` **no cuenta** las autorreferencias a propósito: `visit_fn_decl` fija
  `cur_fn_id` y `add` retorna temprano si `id == cur_fn_id`. `VarInfo` lo documenta.
- El `Dropper`, que descuenta usos al borrar un nodo, **no tenía esa exclusión**. Al borrar
  `var self = handler`, hacía `drop_usage(handler)`: `usage` pasaba de 1 a 0.
- En la pasada siguiente, `can_drop_binding` veía `usage == 0 && assign == 0` y borraba la
  declaración, aunque `setTimeout(handler, 0)` la seguía usando.

**Fix.** El `TreeShaker` y el `Dropper` rastrean `cur_fn_id` / `cur_class_id` con el mismo
push/pop que el `Analyzer` (incluida la forma de `visit_class_decl`: `super_class` afuera
de la binding, `decorators` y `body` adentro), y el `Dropper` saltea `drop_usage` en
autorreferencias. El descuento ahora espeja exactamente la suma.

**Alternativa descartada:** contar la autorreferencia en el `Analyzer`. Dejaría viva toda
función que solo se llama a sí misma: regresión de DCE.

**Alcance que salió de más:** el mismo mecanismo rompía `class`. Se midió con un test
antes de incluirlo (falló sin el fix con `ReferenceError: Widget is not defined`), y
recién ahí se sumó al título.

---

## 2. ⭐ La hipótesis del handoff apuntaba al crate equivocado; el log lo resolvió en una corrida

El handoff (`difuzz-swc-estado-2026-09-21.md` §8.7) sugería `used_recursively` o
`subtract_cycles`, pensando en el usage analyzer del minifier. El mensaje real salió del
tree-shaker de **otro crate**:

```
DEBUG Dropping function `handler#3` as it's not used     ← simplify/dce/mod.rs:1079
```

Cómo se prendió (leído del crate, no recordado): `report_change!` y los `debug!` exigen
`debug_assertions` + feature `debug`; `scripts/exec.sh` ya corre con `RUST_LOG=trace`.

```bash
RUST_LOG=debug cargo test -q --features debug --test exec <filtro> 2>&1 | tee /tmp/x.log
grep -n "<ident>" /tmp/x.log | grep -i -E "drop|remov|unused"
```

**Regla:** antes de leer código siguiendo una hipótesis, **correr el repro con logs y
buscar el ident**. Un `grep` del mensaje en el código da archivo:línea de la pasada
culpable. Esto le ganó a una hora de lectura dirigida por la hipótesis.

---

## 3. ⛔ *"Successfully rebased"* rompió `exec.rs` sin avisar

#12398 se mergeó mientras #12401 estaba abierta. Las dos agregaban tests **al final** de
`tests/exec.rs`. GitHub marcó `DIRTY CONFLICTING` (por eso no corrió ningún workflow: sin
merge commit posible no hay CI). `git rebase upstream/main` terminó limpio… y el archivo
quedó así:

```rust
        for ([i.n] of [[6]]);
        console.log(i.n);
fn issue_12400_fn_decl_self_ref_in_unused_local() {   // ← adentro del r#"…"# de #12397
```

Git se comió el cierre del test de #12397 (`"#, ); }`) y el `#[test]` del primero de los
míos. El raw string quedó abierto, así que **mi test entero pasó a ser JS dentro del test
de #12397**, y ese test quedó corrupto. Compilaba.

**Cómo se detectó:** `test result: ok. 1 passed` donde el filtro debía matchear 2.

**Cómo se arregló:** sin reparar a mano algo cuyo final no conocía:

```bash
git checkout upstream/main -- <archivo>          # la versión de upstream, entera
cat tests-propios.txt >> <archivo>               # re-agregar lo mío
git diff upstream/main --numstat -- <archivo>    # tiene que dar: N  0
```

**Reglas:**
- Después de un rebase donde upstream tocó un archivo que el commit también toca:
  `git diff upstream/main --numstat` sobre ese archivo. Si el commit solo agrega, el
  segundo número es **0**.
- **Contar** los tests que matchean el filtro antes de empujar. `ok` no alcanza: `1 passed`
  cuando esperabas 2 es un rojo.
- La base cambió (entraron #12392 y #12398): suite completa de nuevo, con `--no-fail-fast`.

---

## 4. La base de comparación de difuzz tiene que ser la del mismo binario

El scratch `~/difuzz-main` corre un binario **anterior** a `paren_remover`, `known.rs` y
`--jobs`. Comparé su salida (`DIVERGENCIAS=18`) contra la corrida del 22-sep (9, binario
nuevo) y parecía que el fix destapaba casos. La base correcta era la del 21-sep (19,
mismo binario): 19 − 18 = exactamente `unused_keep_harmony_destructuring`.

También falló la invocación: el binario viejo no tiene `--jobs` ni `--help` (toma
`<corpus_dir>` posicional). ⇒ Antes de invocar un binario de difuzz que no sea el de
`~/difuzz`, leer su `uso:` primero.

**Resuelto (noche del 22-sep):** `total=3119` contra `3102` es el **árbol**, no el
binario. El binario nuevo (`~/difuzz-main2`) sobre `tests/` de `4b42cd59d7` también da
`total=3119`. La corrida del 22-sep (`~/difuzz-out-4`, 3102) usó otro árbol; cuál, NO SE.
No cambia ningún veredicto: las dos dieron las mismas 9 divergencias.

---

## 5. Pre-flight de territorio con `ls-remote` + PRs abiertas del dueño

El usage analyzer es zona de Austaras. Dos comandos dieron el mapa:

```bash
git ls-remote --heads https://github.com/Austaras/swc.git | grep -i -E "<términos>"
gh pr list --repo swc-project/swc --author Austaras --state open
```

Salió #12392 (*refactor(es/minifier): Dead code*). El título asustaba; **los archivos**
(`gh pr view --json files`) mostraron que no tocaba `simplify/dce`. Se mergeó el mismo día
y el rebase lo absorbió sin conflicto de código. ⇒ Decidir territorio por archivos, no por
títulos.

---

## 6. Lo aplicado de lecciones anteriores

- Changeset copiado de los **precedentes sobre el mismo archivo** (`git show <commit> --
  .changeset/`), no del recuerdo: `swc_core` + `swc_ecma_transforms_optimization`,
  `patch`. #12106 sumaba `swc_ecma_minifier` porque tocaba ese crate; acá no.
- Tipo y scope por ruta (§1.19): los tres fixes previos de ese archivo usan
  `fix(es/minifier):` aunque el crate sea otro.
- Tests que fallan sin el fix, verificados antes de aplicar el parche, y dicho en el body.
- Suites completas: `swc_ecma_minifier` (9 líneas `test result` contando doctests; `exec`
  504, `compress` 3115, sin cambios de snapshot) y `swc_ecma_transforms_optimization`.
- `--force-with-lease` para el push post-rebase, corrido por el humano.

---

## 7. Errores propios de la sesión

1. **`gh pr view --comments --json`**: flags incompatibles. Un comando de lectura mal
   armado es barato; en uno que escribe, no. ⇒ Los de escritura se arman aparte.
2. **`awk '{print $2}'` sobre `gh pr checks`**: los nombres tienen espacios. ⇒ Siempre
   `--json` + `--jq` para salida de `gh`.
3. **Loop de changesets con `git show HEAD:`**: los changesets se borran en cada release;
   buscar en `HEAD` no encuentra nada. Salida vacía no es evidencia.
4. **"Espera 505"** en `exec`: conté dos veces mis propios tests. Eran 504.
5. **"Espera 8 delimitadores"** en el body de #12404: eran tres bloques, 6.
6. **Comandos de "después de 1 minuto" en el mismo bloque** que el push: corrieron pegados
   y mostraron el SHA viejo. Repite el error §7.1 de #12398. ⇒ Lo que requiere esperar, en
   un bloque aparte, siempre.
7. **Invocación de difuzz sin leer su `uso:`** (§4).
8. **`gh pr checks 12401` desde `~/difuzz`** (noche): sin `-R`, `gh` resuelve el repo del
   directorio actual y responde *Could not resolve to a PullRequest*. Los `gh api` con ruta
   completa sí andaban. ⇒ Todo `gh pr`/`gh issue` sobre SWC lleva `-R swc-project/swc`.
9. **La base de comparación salió de un doc, no de disco** (noche). El mapa decía que la
   base del 22-sep vivía en `~/difuzz/difuzz-out/div`; tenía 34 entradas del 21-sep. La real
   era `~/difuzz-out-4/div` (9). ⇒ Antes de comparar contra una base: `stat` + conteo +
   lista, y cotejar con la tabla de divergencias.

---

## 8. Los números

| Métrica | Valor |
|---|---|
| Líneas | +143 / −15, 3 archivos (dce, exec tests, changeset) |
| Tests de regresión | 2 exec tests (función y clase), ambos rojos sin el fix |
| Snapshots cambiados | 0 |
| `swc_ecma_minifier` | 0 failed; `exec` 504, `compress` 3115, `mangle` 2219, `terser_exec` 2128 |
| difuzz (binario viejo) | 19 → 18, solo sale el caso del issue |
| Issue → PR | mismo día |
| Rebases | 1 (por merge de #12398), con reparación de `exec.rs` |

---

## 9. Checklist destilado (agregar al del minifier)

- [ ] ¿Corrí el repro con `--features debug` + `RUST_LOG=debug` antes de seguir la hipótesis?
- [ ] ¿El `grep` del mensaje del log me dio archivo:línea de la pasada?
- [ ] Después de rebase: ¿`git diff upstream/main --numstat` da `N 0` en los archivos compartidos?
- [ ] ¿Conté los tests que matchean el filtro, no solo `ok`?
- [ ] ¿La base de comparación de difuzz es del mismo binario?
- [ ] ¿Los comandos que requieren esperar están en un bloque aparte?
- [ ] ¿Todo `gh pr`/`gh issue` sobre SWC lleva `-R swc-project/swc`?
- [ ] ¿La base de comparación la verifiqué en disco (`stat`, conteo, lista), no en un doc?

---

*Todo lo afirmado se verificó con salida de comandos de la sesión o leyendo `main` @
`4b42cd59d7` / `42e4f7e98b`, salvo donde dice NO VERIFICADO. Complementa a
`lecciones-pr-12398-swc.md`, `swc-regla-PR-para-kdy1.md` y `METODO_TRANSVERSAL.md`.*
