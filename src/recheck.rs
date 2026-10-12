//! Re-chequeo de los flags estaticos `with` y `global_conocido` (M13, los
//! pide la politica de oxc: backend::Politica::rechequeos; pendiente del
//! fuego). Como `extends` (clasificar::extends_matters): un flag
//! estatico mira todo el programa, no la divergencia. Antes iban a `known/` sin
//! verificar que la divergencia viniera de eso.
//!
//! - `global_conocido`: se corren el original y el minificado de nuevo con esos
//!   globals definidos como stubs inertes (preload, DIFUZZ_GLOBALS). Si la
//!   divergencia sigue, los globals no la explican. Sin re-minificar: 2 node.
//! - `with`: cada `with (o) S` pasa a `{ o; S; }` (se evalua `o`, sin el scope),
//!   se minifica igual y se compara. Si la divergencia sigue, `with` no la
//!   explica. 1 minify + 2 node.
//!
//! - violaciones atrapadas (b4-a, R1-H04 / C3): el preload ubica cada TDZ, coercion
//!   de Symbol y `class extends` invalido que lanzo en A (atrapado o no); se cambia
//!   cada una por algo que no lanza, se minifica igual y se compara. 1 minify + 2 node.
//!
//! Los re-chequeos solo pueden SACAR un flag (known -> div), nunca ponerlo.
//!
//! b1-b (R1-H01/H02, R2-H02): un re-chequeo solo deja el flag (known/) si es
//! concluyente (`veredicto`). Antes, ante cualquier duda (no parsea, el
//! original no termina, el original modificado toma otro camino) el flag se
//! conservaba y un bug real podia terminar en known/. Ahora la duda va a div/
//! con una nota en meta.txt (`Recheck::Inconcluso`).
//!
//! n-principio (09-oct, noche): `extends`, `with`, `global_conocido`, las violaciones y
//! solo sloppy tambien rigen por `veredicto_neutralizado` (B == A'). `veredicto` (con
//! `mismo_camino` y `localizada`) mandaba a known/ un bug real en la rama que solo corre
//! con la violacion (el catch de la TDZ, la rama sin `window`, la rama sloppy, la rama
//! que depende del scope del `with`): MEDIDO, 17 casos y 116 filas de 8 celdas de oxc
//! (noche2/n-principio/corpus-ataque). Se borraron `veredicto`, `mismo_camino` y
//! `localizada`: ya nadie los usa.

use std::panic::{catch_unwind, AssertUnwindSafe};

use swc_common::{sync::Lrc, BytePos, FileName, Globals, SourceMap, Spanned, DUMMY_SP, GLOBALS};
use swc_ecma_ast::{BlockStmt, Class, Expr, ExprStmt, Lit, Null, Program, Stmt};
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};
use swc_ecma_transforms_base::fixer::fixer;
use swc_ecma_visit::{VisitMut, VisitMutWith};

use crate::minify::{self, SourceKind};
use crate::{backend, oracle, runner};

/// b1-b: resultado de un re-chequeo de asuncion. Solo `Explica` conserva el flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recheck {
    /// La asuncion explica la divergencia: el flag se conserva (known/).
    Explica,
    /// Con la asuncion neutralizada la divergencia sigue: el flag se va (div/).
    NoExplica,
    /// No se pudo saber: el flag se va (div/) y la razon va a la nota de meta.txt.
    Inconcluso(String),
}

/// Paso 7 (revision-tint B1): el veredicto de los re-chequeos que NEUTRALIZAN el codigo
/// en el que vive la asuncion (`builtins`: los pisados quedan fijos y el codigo que los
/// pisa no corre; `coercion`: los helpers renombrados no corren; contenido: toString y
/// .stack fijos; `profundidad`: con mas pila no se llega al limite). Si el bug esta
/// adentro de ese codigo, A' y B' lo saltean los dos: B' == A' no prueba nada, y
/// `mismo_camino` ni `localizada` tampoco (MEDIDO: revision-tint/corpus-adv, 12 de 15 a
/// known/ con `veredicto`). Es concluyente solo si:
/// 1. A' y B' terminan y B' da lo mismo que A' (la divergencia desaparece), y
/// 2. B da lo mismo que A': el minificado de verdad ya se comporta como el original con
///    la asuncion neutralizada. Si el codigo neutralizado tuviera un bug que B ejecuta,
///    B no se comportaria como un programa que no lo ejecuta, salvo coincidencia exacta.
///
/// Sin `mismo_camino`: un built-in pisado que devuelve lo mismo que el original
/// (`includes = function () { return Math.cbrt(-8) < 0; }`) da A' == A y el bug de B
/// quedaba tapado (corpus de aceptacion, 20-p01-builtins-mismo-camino).
/// `salada`: solo para contenido, la corrida de A con el contenido salado; habilita la
/// variante con marca de `localizada` (`localizada_con_marca`).
///
/// n-principio: rige tambien para `extends`, `with`, `global_conocido`, las violaciones
/// y solo sloppy. Ahi la neutralizacion no apaga el codigo de la asuncion pero cambia el
/// camino: el codigo que solo corre CON la violacion (el catch de la TDZ, la rama sin
/// `window`, la rama sloppy) no corre en A' ni en B'. Con `veredicto` (que aceptaba A' ==
/// A, o B distinto de A solo en lineas que A' tambien cambia) un bug ahi iba a known/:
/// n01 (`extends 1` y `Math.cbrt(-8)` en el catch: A=-2, B=NaN, A'=function), n24 (A' ==
/// A por construccion). Positivos que pierde (van a div/, gana el principio): los que
/// dan en B algo que no es ni A ni A' (issue_1105 con `with`: oxc pasa `0/0` a `NaN`, que
/// dentro del `with` resuelve a `o.NaN`; dead_code de terser con funciones en bloque).
///
/// b7-1 (revision n2, IMPORTANTE 1): B == A' tambien puede ser una coincidencia. Si el bug
/// deja justo el valor que la variable tiene cuando la rama de la violacion no corre (`""`,
/// `false`, `0`, un acumulador sin cambios), B corre esa rama con el bug y da lo mismo que A',
/// que no la corre (MEDIDO con oxc 0.153.0: 11 programas, 72 filas de 8 celdas en known/).
/// Comparar salidas no lo distingue: hace falta saber si B paso por la violacion. Por eso,
/// ademas, `camino` (que solo se llama si todo lo anterior da Explica) tiene que dar
/// `Camino::Igual`: B no pasa por la violacion (el sensor no la ve en B) o B no cambia de
/// camino cuando se neutraliza la asuncion EN B (`comparar_caminos`: cobertura de bloques de
/// V8 de B con y sin la neutralizacion). Si el minifier aprovecho la asuncion, en B ya no hay
/// nada que dependa de ella y el camino no cambia; si B todavia depende y la salida coincide,
/// la coincidencia no prueba nada: Inconcluso (div/).
pub fn veredicto_neutralizado(
    a: &runner::Exec,
    b: &runner::Exec,
    a2: &runner::Exec,
    b2: &runner::Exec,
    salada: Option<&runner::Exec>,
    camino: impl FnOnce() -> Camino,
) -> Recheck {
    if !a2.completed() {
        return Recheck::Inconcluso(format!("el original modificado no termino ({:?})", a2.status));
    }
    if matches!(b2.status, runner::Status::Timeout | runner::Status::Failed(_)) {
        return Recheck::Inconcluso(format!("el minificado modificado no termino ({:?})", b2.status));
    }
    if !matches!(oracle::compare(a2, b2), oracle::Verdict::Same) {
        return Recheck::NoExplica;
    }
    let explica = matches!(oracle::compare(a2, b), oracle::Verdict::Same) || salada.is_some_and(|s| localizada_con_marca(a, b, a2, s));
    if !explica {
        return Recheck::Inconcluso(
            "con la asuncion neutralizada la divergencia desaparece, pero el minificado no se comporta como el original neutralizado (B != A'): el codigo neutralizado puede tener el bug".into(),
        );
    }
    match camino() {
        Camino::Igual => Recheck::Explica,
        Camino::Distinto(n) => Recheck::Inconcluso(format!(
            "con la asuncion neutralizada la divergencia desaparece y B se comporta como el original neutralizado, pero B todavia pasa por la violacion ({n}): la coincidencia no prueba nada, el codigo que solo corre con la violacion puede tener el bug"
        )),
        Camino::NoSe(n) => Recheck::Inconcluso(format!(
            "con la asuncion neutralizada la divergencia desaparece y B se comporta como el original neutralizado, pero no pude ver si B pasa por la violacion ({n})"
        )),
    }
}

/// b7-1: si B pasa por la violacion que el re-chequeo neutraliza (`veredicto_neutralizado`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Camino {
    /// B no pasa por la violacion: el sensor no la ve en B, o B no cambia de camino con la
    /// asuncion neutralizada.
    Igual,
    /// B pasa por la violacion (el detalle va a la nota).
    Distinto(String),
    /// No se pudo saber (el detalle va a la nota).
    NoSe(String),
}

/// b7-1: compara el camino de dos corridas del MISMO texto con cobertura (`run_con_cobertura`):
/// `con` (B con la asuncion como en B) y `neutro` (B con la asuncion neutralizada). Antes se
/// pide que `con` se comporte como `b` (la corrida de B que se clasifica) y, si hay, que
/// `neutro` se comporte como `b2` (B'): si no, la cobertura no es la de esas corridas
/// (no determinismo, o la instrumentacion cambio el programa) y no se sabe.
///
/// b7-1b (revision 7.1, H2): antes que el camino, el sensor de uso (lineas `uso` y `uso?` del
/// canal de las dos corridas, preload.js): mirar el camino no ve una dependencia por datos (la
/// asuncion elige un indice, una clave o un valor y B recorre el mismo codigo con y sin ella).
/// Si B toco la asuncion, B == A' puede ser coincidencia: `Distinto`; si no se pudo sensar,
/// `NoSe`.
pub fn comparar_caminos(b: &runner::Exec, con: &runner::Exec, b2: Option<&runner::Exec>, neutro: &runner::Exec) -> Camino {
    let mut usos: Vec<&str> = Vec::new();
    let mut no_sensados: Vec<&str> = Vec::new();
    for l in con.ctl.lines().chain(neutro.ctl.lines()) {
        if let Some(u) = l.strip_prefix("uso ") {
            if !usos.contains(&u) {
                usos.push(u);
            }
        } else if let Some(u) = l.strip_prefix("uso? ") {
            if !no_sensados.contains(&u) {
                no_sensados.push(u);
            }
        }
    }
    if !usos.is_empty() {
        return Camino::Distinto(format!("B usa la asuncion: {}", usos.join(", ")));
    }
    if !no_sensados.is_empty() {
        return Camino::NoSe(format!("no pude sensar si B usa la asuncion: {}", no_sensados.join(", ")));
    }
    if !matches!(oracle::compare(b, con), oracle::Verdict::Same) {
        return Camino::NoSe(format!("con la cobertura prendida B no da lo mismo ({:?})", con.status));
    }
    if b2.is_some_and(|b2| !matches!(oracle::compare(b2, neutro), oracle::Verdict::Same)) {
        return Camino::NoSe(format!("con la cobertura prendida B' no da lo mismo ({:?})", neutro.status));
    }
    match (con.cobertura(), neutro.cobertura()) {
        (Some(x), Some(y)) if x == y => Camino::Igual,
        (Some(x), Some(y)) => Camino::Distinto(format!(
            "la cobertura de bloques de V8 de B cambia con la asuncion neutralizada en B: {x} contra {y}"
        )),
        _ => Camino::NoSe("una corrida de B no dejo cobertura de V8".into()),
    }
}

/// b7-1: el camino de B cuando la neutralizacion es del entorno (stubs de globals, built-ins
/// fijos, contenido fijo, mas pila): B y B' corren el mismo texto, asi que se corre B dos
/// veces con cobertura, sin y con la neutralizacion (`args`/`env`), y se comparan. 2 node.
///
/// b7-1b (revision 7.1, H1): el texto que corre es B con los contadores de rama
/// (`instrumentar_ramas`), porque V8 no separa con bloques el lado derecho de `&&=`/`||=`/`??=`
/// ni los valores por defecto (oxc hace `t&&=e=...` de un `if (t) t = (e = ...)`). `con_env`:
/// lo que lleva ademas la corrida sin neutralizar (el sensor de uso de los built-ins).
pub fn camino_entorno(
    r: &runner::Runner,
    minified: &str,
    b: &runner::Exec,
    b2: &runner::Exec,
    (args, env): (&[&str], &[(&str, &str)]),
    con_env: &[(&str, &str)],
) -> Camino {
    let (texto, ganchos) = match texto_instrumentado(minified, crate::analisis::ramas(minified, r.kind())) {
        Ok(t) => t,
        Err(c) => return c,
    };
    let con = r.run_con_cobertura(&texto, &[], &con_ganchos(con_env, ganchos));
    let neutro = r.run_con_cobertura(&texto, args, &con_ganchos(env, ganchos));
    comparar_caminos(b, &con, Some(b2), &neutro)
}

/// b7-1c (revision 7.1b, S2): el preload define los ganchos (`__difuzzNeutroRama`, etc.) y saca lo
/// insertado del texto de las funciones solo con DIFUZZ_GANCHOS=1, que va solo cuando el texto que
/// corre esta instrumentado. Antes los definia en toda corrida con cobertura: un B sin instrumentar
/// podia verlos.
pub const GANCHOS: (&str, &str) = ("DIFUZZ_GANCHOS", "1");

/// b7-1c: `env` mas `GANCHOS` si `ganchos`.
fn con_ganchos<'a>(env: &[(&'a str, &'a str)], ganchos: bool) -> Vec<(&'a str, &'a str)> {
    let mut v = env.to_vec();
    if ganchos {
        v.push(GANCHOS);
    }
    v
}

/// b7-1b: el texto que corre con cobertura: el instrumentado, o B tal cual si no hacia falta
/// instrumentar nada; si no se pudo, no se sabe. b7-1c: y si esta instrumentado (`GANCHOS`).
fn texto_instrumentado(minified: &str, r: Result<Option<Option<String>>, String>) -> Result<(String, bool), Camino> {
    match r {
        Ok(Some(Some(t))) => Ok((t, true)),
        Ok(Some(None)) => Ok((minified.to_string(), false)),
        Ok(None) => Err(Camino::NoSe("no pude instrumentar el minificado (no parsea con SWC)".into())),
        Err(e) => {
            crate::analisis::registrar("re-chequeo (instrumentar el minificado)", &e);
            Err(Camino::NoSe(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default())))
        }
    }
}

/// b7-1: el camino de B cuando la neutralizacion cambia el texto (`with`, coercion): B
/// instrumentado (`instrumentado`, de `interruptores_with`/`interruptores_coercion`) corre
/// con los interruptores apagados (DIFUZZ_NEUTRO=0: se comporta como B) y prendidos (=1:
/// neutralizan EN B lo mismo que el re-chequeo en el original). Mismo texto: se comparan
/// las coberturas. 2 node.
pub fn camino_interruptores(r: &runner::Runner, b: &runner::Exec, instrumentado: Result<Option<Option<String>>, String>) -> Camino {
    let texto = match instrumentado {
        Ok(Some(Some(t))) => t,
        // B ya no tiene nada que neutralizar (el minifier lo saco): nada en B depende de eso.
        Ok(Some(None)) => return Camino::Igual,
        Ok(None) => return Camino::NoSe("no pude instrumentar el minificado (no parsea con SWC)".into()),
        Err(e) => {
            crate::analisis::registrar("re-chequeo (instrumentar el minificado)", &e);
            return Camino::NoSe(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()));
        }
    };
    let con = r.run_con_cobertura(&texto, &[], &[("DIFUZZ_NEUTRO", "0"), GANCHOS]);
    let neutro = r.run_con_cobertura(&texto, &[], &[("DIFUZZ_NEUTRO", "1"), GANCHOS]);
    comparar_caminos(b, &con, None, &neutro)
}

/// b7-1: el camino de B para las violaciones que ubica el sensor (TDZ, Symbol, `extends`):
/// las ve el sensor en B (lineas `viol` del canal de B). `clases`: las que cuentan (`None`,
/// todas). Sin el sensor entero en B (`con_sensor` falso o `sensor_degradado`) no se sabe.
pub fn camino_sensor(b: &runner::Exec, con_sensor: bool, clases: Option<&[&str]>) -> Camino {
    if !con_sensor {
        return Camino::NoSe("B corrio sin el sensor de violaciones".into());
    }
    if let Some(d) = b.sensor_degradado() {
        return Camino::NoSe(format!("el sensor de violaciones no corrio entero en B ({d})"));
    }
    let en_b: Vec<&str> = violaciones(b)
        .into_iter()
        .filter(|s| clases.is_none_or(|c| c.iter().any(|k| s.split(' ').next() == Some(*k))))
        .collect();
    if en_b.is_empty() {
        Camino::Igual
    } else {
        Camino::Distinto(format!("el sensor ve en B la violacion {}", en_b.join(", ")))
    }
}

/// Integracion 7.1 + 7.4 (H3 de rev-7.1, H1/H2 de rev-int): el camino de B cuando A y B se
/// compararon sin el sensor (I3: alguno paso el tope o se cayo con el). B (`b`, sin el sensor)
/// no tiene sensor que mirar, asi que se corre B otra vez CON el sensor (tope
/// `runner::MAX_PAUSAS_MAYOR`) y cobertura, y otra SIN el sensor con cobertura. La corrida con el
/// sensor da el camino solo si: corrio entera (sin `sensor_tope` ni `sensor_caido`), se comporta
/// como `b` (salida y estado, `oracle::compare`), la de sin sensor tambien, y las dos tienen la
/// MISMA cobertura de bloques de V8. Si no, no se sabe (div/): un programa que nota el sensor
/// (cada pausa llama a su `Error.prepareStackTrace`) y toma otro camino con el no puede
/// prestarle a B un camino que B sin el sensor no toma (rev-int H1, MEDIDO). 2 node, y solo
/// cuando todo lo demas ya daba Explica (closure de `veredicto_neutralizado`).
/// Integracion 7.1b: las dos corridas son del minificado con los contadores de rama de 7.1b
/// (`texto_instrumentado` + `analisis::ramas`, lo mismo que `camino_entorno`): la cobertura de V8
/// no separa el lado derecho de `&&=`/`||=`/`??=` ni un valor por defecto, y un programa que nota el
/// sensor puede cambiar de camino solo ahi (MEDIDO: b7-int/sensor-rhs a01 iba a known/).
pub fn camino_sensor_i3(sin: &runner::Runner, con: &runner::Runner, minified: &str, b: &runner::Exec, clases: Option<&[&str]>) -> Camino {
    // Integracion 7.1c: el texto instrumentado corre con los ganchos (`con_ganchos`), como en
    // `camino_entorno`; el hash de la cobertura lo arma el preload con lo capturado al inicio (S2).
    let (texto, ganchos) = match texto_instrumentado(minified, crate::analisis::ramas(minified, sin.kind())) {
        Ok(t) => t,
        Err(c) => return c,
    };
    // 8.7: con el sensor entero (sin el pre-filtro perezoso): perezosa, una TDZ que B atrapa con
    // algo que `puede_atrapar` no ve en el texto no dejaba `viol` y el camino daba Igual.
    let c = con.sensor_entero().run_con_cobertura(&texto, &[], &con_ganchos(&[("DIFUZZ_MAX_PAUSAS", runner::MAX_PAUSAS_MAYOR)], ganchos));
    if let Some(d) = c.sensor_degradado() {
        return Camino::NoSe(format!(
            "A y B se compararon sin el sensor (I3) y B con el sensor (tope {}) tampoco corrio entero ({d})",
            runner::MAX_PAUSAS_MAYOR
        ));
    }
    if !matches!(oracle::compare(b, &c), oracle::Verdict::Same) {
        return Camino::NoSe(format!("con el sensor B no da lo mismo que sin el ({:?})", c.status));
    }
    let s = sin.run_con_cobertura(&texto, &[], &con_ganchos(&[], ganchos));
    if !matches!(oracle::compare(b, &s), oracle::Verdict::Same) {
        return Camino::NoSe(format!("con la cobertura prendida B no da lo mismo ({:?})", s.status));
    }
    match (s.cobertura(), c.cobertura()) {
        (Some(x), Some(y)) if x == y => camino_sensor(&c, true, clases),
        (Some(x), Some(y)) => Camino::NoSe(format!(
            "con el sensor B toma otro camino que sin el (cobertura de bloques de V8: {y} contra {x})"
        )),
        _ => Camino::NoSe("una corrida de B no dejo cobertura de V8".into()),
    }
}

/// Integracion 7.1 + 7.4: el camino de B por el sensor. `i3`: el runner con el sensor y el
/// minificado, si A y B se compararon sin el sensor (I3); `r` es entonces el runner sin el.
/// 8.7: si B corrio con el sensor perezoso (`runner::puede_atrapar(minified)` dio false, ver
/// `Runner::run_inner`), sus lineas `viol` no son las del sensor entero: un falso negativo del
/// pre-filtro (algo que atrapa armado en runtime) mandaba a known/ un bug en el catch de la
/// violacion (MEDIDO, 8.7/repro p01). Se busca como en I3: B otra vez con el sensor entero y
/// sin el, con cobertura (`camino_sensor_i3`). 2 node, solo cuando lo demas ya daba Explica.
pub fn camino_de_b(r: &runner::Runner, i3: Option<&runner::Runner>, minified: &str, b: &runner::Exec, clases: Option<&[&str]>) -> Camino {
    match i3 {
        Some(con) if !r.con_sensor() => camino_sensor_i3(r, con, minified, b, clases),
        _ if r.con_sensor() && !runner::puede_atrapar(minified) => camino_sensor_i3(&r.sin_sensor(), r, minified, b, clases),
        _ => camino_sensor(b, r.con_sensor(), clases),
    }
}

/// b7-1: el camino de B para solo sloppy: B corre en sloppy y en estricto con prefijos del
/// mismo largo (`PREFIJO_SLOPPY`, una directiva desconocida, y `PREFIJO_ESTRICTO`), asi los
/// offsets de la cobertura coinciden. Si B no compila en estricto se prueba con el binding
/// `arguments` renombrado (como el original); si tampoco, no se sabe. 2 node (+ `node --check`).
pub fn camino_estricto(r: &runner::Runner, minified: &str, b: &runner::Exec) -> Camino {
    let mut base = std::borrow::Cow::Borrowed(minified);
    if !r.node_accepts(&format!("{PREFIJO_ESTRICTO}{minified}")) {
        match crate::analisis::sin_arguments(minified) {
            Ok(Some(ren)) if r.node_accepts(&format!("{PREFIJO_ESTRICTO}{ren}")) => base = std::borrow::Cow::Owned(ren),
            Err(e) => {
                crate::analisis::registrar("re-chequeo sloppy (camino de B)", &e);
                return Camino::NoSe(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()));
            }
            _ => return Camino::NoSe("el minificado no compila en estricto".into()),
        }
    }
    // b7-1b (revision 7.1, H1/H2 d07): con los contadores de rama y el sensor del `this` de
    // sloppy (`interruptores_sloppy`): en estricto, un `this` que sloppy cambia es uso.
    let (texto, ganchos) = match texto_instrumentado(&base, crate::analisis::interruptores_sloppy(&base)) {
        Ok(t) => t,
        Err(c) => return c,
    };
    let env = con_ganchos(&[], ganchos);
    let con = r.run_con_cobertura(&format!("{PREFIJO_SLOPPY}{texto}"), &[], &env);
    let neutro = r.run_con_cobertura(&format!("{PREFIJO_ESTRICTO}{texto}"), &[], &env);
    comparar_caminos(b, &con, None, &neutro)
}

/// b7-1: prefijos de `camino_estricto`, del mismo largo.
pub const PREFIJO_SLOPPY: &str = "\"use sloppy\";\n";
pub const PREFIJO_ESTRICTO: &str = "\"use strict\";\n";

/// Paso 7: lo que el preload pone con DIFUZZ_SALT=fijo (preload.js, toString y
/// prepareStackTrace) y con DIFUZZ_SALT=1 (la sal). `localizada_con_marca` los busca.
pub const FIJO_TOSTRING: &str = "function () { [native code] }";
pub const FIJO_MARCO: &str = "    at dz";
pub const SAL_TOSTRING: &str = "/*dz*/";

/// Paso 7 (revision-tint B1, contenido): `localizada` con marca. Ademas de lo de
/// `localizada` (B termina, A y A' lanzan los dos o ninguno, mismas lineas, A' cambia en
/// cada linea en que B difiere de A), cada una de esas lineas tiene que MOSTRAR contenido:
/// - un marco fijo: A' es exactamente `FIJO_MARCO` y A y B tambien son lineas de marco
///   con la forma de V8 (`es_marco`); n-principio: y el mismo marco salvo la posicion
///   (`cabeza_marco`);
/// - un toString fijo: A' contiene `FIJO_TOSTRING`, A y B coinciden con A' fuera de cada
///   aparicion (el resto de la linea es igual), y en A con el contenido salado esa misma
///   linea trae la sal (el contenido de A fluye a esa linea, no solo decide un camino).
///
/// r07/r08 (corpus-adv: el bug en la rama que el contenido fijo apaga) no pasan: la
/// linea en A' es `none`, sin marca. Lo que sigue pasando (riesgo anotado): un programa
/// que imprima a proposito una linea con forma de marco o el toString entero en la rama
/// del contenido fijo.
pub fn localizada_con_marca(a: &runner::Exec, b: &runner::Exec, a2: &runner::Exec, salada: &runner::Exec) -> bool {
    let lines = |e: &runner::Exec| oracle::nombres_de_celda(&e.stdout).lines().map(String::from).collect::<Vec<_>>();
    let (la, lb, l2, ls) = (lines(a), lines(b), lines(a2), lines(salada));
    let throw = format!("{}throw ", runner::DZ);
    let cubre = |i: usize, x: &str, y: &str, z: &str| {
        if z.starts_with(&throw) || x == z {
            return false;
        }
        if z == FIJO_MARCO {
            // n-principio (n-ruido r02/r03/r09): el mismo marco salvo la posicion. Antes valia
            // cualquier par de marcos, y un bug que cambia el nombre (`at pasa` -> `at falla`), el
            // orden o la cuenta de marcos (con `stackTraceLimit` = 10 se ve como nombres
            // corridos) iba a known/ en carril A.
            return es_marco(x) && es_marco(y) && cabeza_marco(x) == cabeza_marco(y);
        }
        if !z.contains(FIJO_TOSTRING) {
            return false;
        }
        let partes: Vec<&str> = z.split(FIJO_TOSTRING).collect();
        // La linea salada solo se alinea con la de A si la sal no agrego lineas.
        let salada_ok = ls.len() == la.len() && ls[i].contains(SAL_TOSTRING);
        salada_ok && encaja(&partes, x) && encaja(&partes, y)
    };
    b.completed()
        && a.threw() == a2.threw()
        && la.len() == lb.len()
        && la.len() == l2.len()
        && la.iter().zip(&lb).zip(&l2).enumerate().all(|(i, ((x, y), z))| x == y || cubre(i, x, y, z))
}

/// Paso 7: una linea de marco de V8 (`    at f (archivo:3:15)`, `    at archivo:3:15` o
/// `    at Array.map (<anonymous>)`), no cualquier linea que empiece con `    at `.
fn es_marco(l: &str) -> bool {
    let Some(resto) = l.strip_prefix("    at ") else {
        return false;
    };
    if resto.ends_with("(<anonymous>)") {
        return true;
    }
    let resto = resto.strip_suffix(')').unwrap_or(resto);
    let mut p = resto.rsplitn(3, ':');
    let digitos = |s: Option<&str>| s.is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()));
    digitos(p.next()) && digitos(p.next()) && p.next().is_some_and(|s| !s.is_empty())
}

/// n-principio: lo que dice un marco de V8 ademas de su posicion: el nombre (con `new`/
/// `async`/receptor) de `    at f (archivo:3:15)`, vacio para `    at archivo:3:15`, y la linea
/// entera para `    at Array.map (<anonymous>)`. Lo que el minificado cambia sin cambiar el camino
/// es la posicion; un nombre distinto (sin keep_fnames, el mangling de core_defaults) ya no se
/// acepta como contenido: puede ser otra funcion.
fn cabeza_marco(l: &str) -> Option<&str> {
    let resto = l.strip_prefix("    at ")?;
    if resto.ends_with("(<anonymous>)") {
        return Some(resto);
    }
    match resto.rfind(" (") {
        Some(i) if resto.ends_with(')') => Some(&resto[..i]),
        _ => Some(""),
    }
}

/// Paso 7: `s` es `partes` unidas por cualquier texto (cada separador, un comodin).
fn encaja(partes: &[&str], s: &str) -> bool {
    let (Some(primera), Some(ultima)) = (partes.first(), partes.last()) else {
        return false;
    };
    if partes.len() < 2 || s.len() < primera.len() + ultima.len() || !s.starts_with(primera) || !s.ends_with(ultima) {
        return false;
    }
    let mut medio = &s[primera.len()..s.len() - ultima.len()];
    for p in &partes[1..partes.len() - 1] {
        match medio.find(p) {
            Some(j) => medio = &medio[j + p.len()..],
            None => return false,
        }
    }
    true
}

/// b1-b: `Explica` solo si la divergencia desaparece con los globals definidos
/// y el camino se sostiene (`veredicto`).
/// n-principio (n28/n29/n30: el bug en la rama sin `window`, que con el stub no corre):
/// rige `veredicto_neutralizado`, B tiene que dar lo mismo que A'.
pub fn globals_matter(
    r: &runner::Runner,
    kind: SourceKind,
    original: &str,
    minified: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> Recheck {
    // A6: el parseo con SWC va al hijo de analisis (analisis.rs).
    let names = match crate::analisis::globals(original, kind) {
        Ok(n) => n.unwrap_or_default(),
        Err(e) => {
            crate::analisis::registrar("re-chequeo de global_conocido", &e);
            return Recheck::Inconcluso(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()));
        }
    };
    if names.is_empty() {
        return Recheck::Inconcluso("no pude listar los globals (el parser de SWC no lee el original)".into());
    }
    let g = names.join(",");
    let a2 = r.run_with_globals(original, &g);
    if !a2.completed() {
        return Recheck::Inconcluso(format!("el original con stubs de {g} no termino ({:?})", a2.status));
    }
    let b2 = r.run_with_globals(minified, &g);
    // b7-1: B sin y con los stubs, con cobertura (q03/r08: la rama sin `window` con el bug).
    veredicto_neutralizado(a, b, &a2, &b2, None, || camino_entorno(r, minified, b, &b2, (&[], &[("DIFUZZ_GLOBALS", g.as_str())]), &[]))
}

/// b1-b: `Explica` solo si la divergencia desaparece sin el scope del `with` y
/// el camino se sostiene (`veredicto`).
/// n-principio (n23/n24: el bug en la rama que depende del scope): rige
/// `veredicto_neutralizado`. issue_1105 (B no es ni A ni A') pasa a div/.
pub fn with_matters(
    r: &runner::Runner,
    recheck: (backend::Minifier, minify::Preset, SourceKind),
    original: &str,
    minified: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> Recheck {
    let (minifier, preset, kind) = recheck;
    let neutral = match crate::analisis::sin_with(original, kind) {
        Ok(n) => n,
        Err(e) => {
            crate::analisis::registrar("re-chequeo de with", &e);
            return Recheck::Inconcluso(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()));
        }
    };
    let Some(neutral) = neutral else {
        return Recheck::Inconcluso("no pude neutralizar el `with` (no parsea con SWC o no lo encuentro)".into());
    };
    let cfg = preset.build(minifier, kind);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&neutral, &cfg) else {
        return Recheck::Inconcluso("no pude minificar la variante sin `with`".into());
    };
    let a2 = r.run(&o2);
    if !a2.completed() {
        return Recheck::Inconcluso(format!("la variante sin `with` no termino ({:?})", a2.status));
    }
    let b2 = r.run(&m2);
    // b7-1 (q04: la rama que depende del scope, con el bug): B con cada `with (o)` como
    // `with (__difuzzNeutroWith(o))`, con el interruptor apagado y prendido.
    veredicto_neutralizado(a, b, &a2, &b2, None, || camino_interruptores(r, b, crate::analisis::interruptores_with(minified, kind)))
}

/// t-principio: `fn_tostring` y `stack` (paso `contenido` de clasificar). Si A con
/// otro contenido de toString/.stack da otra salida, A depende del contenido, pero
/// eso no dice que la divergencia venga de ahi (un bug ajeno al lado iba a known/).
/// Se corren el original y el minificado con el contenido FIJO (preload,
/// DIFUZZ_SALT=fijo: el mismo texto para cualquier funcion y solo la cabecera del
/// stack), y rige `veredicto`. Sin re-minificar: 2 node.
/// Paso 7 (revision-tint B1, r07/r08): rige `veredicto_neutralizado` con `salada` (la
/// corrida de A con el contenido salado que ya hizo clasificar): B tiene que comportarse
/// como A', o diferir solo en lineas que muestran contenido (`localizada_con_marca`).
pub fn contenido_matters(
    r: &runner::Runner,
    original: &str,
    minified: &str,
    a: &runner::Exec,
    b: &runner::Exec,
    salada: &runner::Exec,
) -> Recheck {
    let env = [("DIFUZZ_SALT", "fijo")];
    let a2 = r.run_with_env(original, &env);
    if !a2.completed() {
        return Recheck::Inconcluso(format!("el original con contenido fijo no termino ({:?})", a2.status));
    }
    let b2 = r.run_with_env(minified, &env);
    // b7-1: B con el contenido de verdad y con el fijo, con cobertura (r05: la rama que
    // depende del texto de la funcion, con el bug).
    veredicto_neutralizado(a, b, &a2, &b2, Some(salada), || camino_entorno(r, minified, b, &b2, (&[], &env), &[]))
}

/// 8.3: los mensajes de V8 que escriben el texto fuente de la expresion llamada, construida
/// o iterada (CallPrinter): `cb is not a function`. Con mangle, el nombre local cambia
/// (`e is not a function`) sin que cambie el comportamiento. Los mas largos primero (contienen
/// a los cortos). Afuera a proposito: `X is not defined` (un global no se manglea: si cambia,
/// es un bug) y `Cannot access 'X' before initialization` (la TDZ tiene su re-chequeo).
const SUFIJOS_V8: &[&str] = &[
    " is not a function or its return value is not async iterable",
    " is not a function or its return value is not iterable",
    " is not a function",
    " is not a constructor",
    " is not async iterable",
    " is not iterable",
];

/// 8.3: un caracter de la expresion que V8 renderiza: identificadores, puntos y el acceso a
/// un nombre privado (`this[#handler]`, como lo escribe V8).
fn de_expr(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$' | '.' | '#' | '[' | ']')
}

/// 9.3 (M2): como escribe V8 una llamada dentro de la expresion (CallPrinter): `(...)`, siempre
/// con tres puntos, sin los argumentos (MEDIDO en node 24.14.1: `f(1, 2)(3)` -> `f(...) is not a
/// function`, `f()()()` -> `f(...)(...)`, `o.f()()` -> `o.f(...)`, `f().g()` -> `f(...).g`,
/// `new (f())()` -> `f(...) is not a constructor`, `f()`x`` -> `f(...)`).
const LLAMADA_V8: &str = "(...)";

/// 9.3 (M2): el acceso opcional como lo escribe V8 (`o?.g()` -> `o?.g is not a function`).
const OPCIONAL_V8: &str = "?.";

/// 8.3: la expresion con que termina `p` (`cb`, `o.cb`, `import_x.f`, `this[#h]`, M2:
/// `f(...)`, `o?.g`) y lo que queda antes.
fn cola_expr(p: &str) -> (&str, &str) {
    let mut corte = p.len();
    loop {
        let r = &p[..corte];
        if r.ends_with(LLAMADA_V8) {
            corte -= LLAMADA_V8.len();
        } else if r.ends_with(OPCIONAL_V8) {
            corte -= OPCIONAL_V8.len();
        } else if let Some(c) = r.chars().next_back().filter(|c| de_expr(*c)) {
            corte -= c.len_utf8();
        } else {
            break;
        }
    }
    (&p[..corte], &p[corte..])
}

/// 8.3: `e` es una expresion renderizada con nombres (no un numero, no vacia, sin punto al
/// principio ni al final, solo caracteres de `de_expr`). M2: con las llamadas (`(...)`) y los
/// accesos opcionales (`?.`) de V8, que no pueden abrir la expresion ni seguir a un punto.
fn es_expr(e: &str) -> bool {
    // `(...)` pasa a `!` y `?.` a `.`: lo demas se mira como antes.
    let n = e.replace(LLAMADA_V8, "!").replace(OPCIONAL_V8, ".");
    !e.contains('!')
        && n.chars().next().is_some_and(|c| !c.is_ascii_digit() && !matches!(c, '.' | '[' | '!'))
        && n.chars().all(|c| de_expr(c) || c == '!')
        && !n.ends_with('.')
        && !n.contains("..")
        && !n.contains(".!")
        && !n.contains("[!")
}

/// 8.3: las dos lineas son el mismo mensaje de V8 salvo la expresion renderizada. Un marco de
/// stack con otro nombre de funcion (`at run` / `at e`, core_defaults) NO entra: n-principio
/// (n-ruido r03) decidio que ese caso va a div/ (`flags_sin_rechequeo_de_punta_a_punta`,
/// `stack-mangling`); cambiarlo es una decision aparte.
fn linea_con_nombre(x: &str, y: &str) -> bool {
    SUFIJOS_V8.iter().any(|s| {
        let (Some(i), Some(j)) = (x.find(s), y.find(s)) else {
            return false;
        };
        let ((px, ex), (py, ey)) = (cola_expr(&x[..i]), cola_expr(&y[..j]));
        x[i + s.len()..] == y[j + s.len()..] && px == py && es_expr(ex) && es_expr(ey) && ex != ey
    })
}

/// 8.3: A y B (normalizados como en `oracle::compare`) tienen las mismas lineas y cada linea
/// en que difieren es un mensaje de V8 que solo cambia en la expresion renderizada
/// (`linea_con_nombre`). Es la precondicion de `mensaje_nombre_matters`: sin ella el
/// re-chequeo no corre (no se paga nada y meta.txt no cambia).
pub fn mensaje_con_nombre_localizado(a: &runner::Exec, b: &runner::Exec) -> bool {
    if a.status != b.status {
        return false;
    }
    let (na, nb) = oracle::normalizadas(a, b);
    let (la, lb): (Vec<&str>, Vec<&str>) = (na.lines().collect(), nb.lines().collect());
    la.len() == lb.len() && la != lb && la.iter().zip(&lb).all(|(x, y)| x == y || linea_con_nombre(x, y))
}

/// 8.3 (`Rechequeo::MensajeNombre`): ¿la divergencia es solo el nombre local que V8 escribe
/// en el mensaje, cambiado por un renombre CORRECTO? Se re-minifica el original con mangle
/// apagado (B0: el mismo compress, sin renombre; `Minifier::build_sin_mangle`) y `Explica`
/// pide las tres cosas:
/// 1. B es alfa-equivalente a B0 (`alfa_equivalentes`, con el resolver de SWC, independiente
///    del de oxc): los mismos nodos salvo los nombres de los bindings, y cada identificador
///    de B resuelve al binding que corresponde al de B0. Un mangle que hace que una
///    referencia capture OTRO binding (el riesgo de propuesta-1: `e is not a function` con
///    `e` otro binding no llamable) no pasa. Si no se puede probar (no parsea, `eval`
///    directo o `with`, una forma que la canonizacion no cubre): `Inconcluso`, div/ con nota.
/// 2. B0 da lo mismo que A (`oracle::compare`): compress solo no diverge.
/// 3. B difiere de B0 solo en esos mensajes (`mensaje_con_nombre_localizado(B0, B)`).
///
/// H1 (revision del bloque 8): la alfa-equivalencia no ve la semantica que depende del
/// nombre de un binding: el `.name` de una funcion o clase sale del binding que la nombra
/// (su nombre propio, o el destino de NamedEvaluation si es anonima: `x = () => {}`,
/// `(x = () => {}) => ...`, `{x = class {}}`, `x ||= function () {}`; `Nombran`). Con un
/// preset que promete keep_names (`Minifier::preserva_nombres`), si el mangle renombro uno
/// de esos bindings el `.name` cambio, y eso ya es un bug de keep_names (oxc 0.153.0 manglea
/// el parametro con default anonimo: oxc_mangler-0.153.0/src/keep_names.rs:84-111 solo mira
/// `Function`, `Class` y `VariableDeclarator`). Si el programa imprime ese `.name` con la
/// forma `X is not a function`, el resto se cumple igual: por eso da `Inconcluso` (div/ con
/// nota) antes de correr B0. Sin keep_names (core_defaults) el cambio del `.name` es una
/// asuncion aceptada ("No Reliance on Function.prototype.name") y no se mira.
///
/// Costo: 1 minificacion (en un hijo), 1 analisis (2 parseos de SWC) y 1 node, solo ante
/// divergencias que pasan la localizacion.
pub fn mensaje_nombre_matters(
    r: &runner::Runner,
    recheck: (backend::Minifier, minify::Preset, SourceKind),
    original: &str,
    minified: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> Recheck {
    let (minifier, preset, kind) = recheck;
    let Some(cfg) = minify::Config::sin_mangle(minifier, preset, kind) else {
        return Recheck::Inconcluso(format!("{} no tiene variante sin mangle", minifier.name()));
    };
    mensaje_nombre_desde(r, &cfg, (kind, minifier.preserva_nombres(preset, kind)), original, minified, a, b)
}

/// 8.3: `mensaje_nombre_matters` con la config sin mangle ya armada (tests: un hijo que pasa
/// el presupuesto). 9.3 (M3): el hijo de `Config::sin_mangle` tiene el mismo timeout y el mismo
/// presupuesto de memoria que el minificador del worker (`subproceso::Remoto::sin_mangle`); si
/// no termina (timeout, memoria, panic) el re-chequeo queda `Inconcluso` con la causa en la nota
/// (div/), nunca known/.
fn mensaje_nombre_desde(
    r: &runner::Runner,
    cfg: &minify::Config,
    nombres: (SourceKind, bool),
    original: &str,
    minified: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> Recheck {
    let m0 = match minify::prepare(original, cfg) {
        minify::Outcome::Ok { minified: m0, .. } => m0,
        minify::Outcome::Panic(m) => {
            return Recheck::Inconcluso(format!("no pude minificar sin mangle: {}", m.lines().next().unwrap_or_default()));
        }
        _ => return Recheck::Inconcluso("no pude minificar sin mangle".into()),
    };
    mensaje_nombre_con(r, nombres, minified, &m0, a, b)
}

/// 8.3: `mensaje_nombre_matters` con el minificado sin mangle (`m0`) ya hecho (tests: un B
/// con un mangle roto a mano). `preserva_nombres`: el preset promete keep_names (H1).
fn mensaje_nombre_con(
    r: &runner::Runner,
    (kind, preserva_nombres): (SourceKind, bool),
    minified: &str,
    m0: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> Recheck {
    if m0 == minified {
        // Sin mangle sale el mismo texto: el nombre distinto no viene del renombre.
        return Recheck::NoExplica;
    }
    match crate::analisis::alfa(m0, minified, kind) {
        Err(e) => {
            crate::analisis::registrar("re-chequeo de mensaje_nombre", &e);
            return Recheck::Inconcluso(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()));
        }
        Ok(None) => return Recheck::Inconcluso("el analisis de difuzz no parsea el minificado o el minificado sin mangle".into()),
        Ok(Some(Err(m))) => {
            return Recheck::Inconcluso(format!(
                "el minificado no es un renombre del minificado sin mangle ({m}): no se puede descartar un bug del mangler"
            ))
        }
        Ok(Some(Ok(nombres))) if preserva_nombres && !nombres.is_empty() => {
            return Recheck::Inconcluso(format!(
                "con keep_names, el mangle renombro un binding del que sale el .name de una funcion o clase ({}): el nombre distinto puede ser un bug de keep_names, no solo el texto del mensaje de V8",
                nombres.join(", ")
            ))
        }
        Ok(Some(Ok(_))) => {}
    }
    let b0 = r.run(m0);
    if matches!(b0.status, runner::Status::Timeout | runner::Status::Failed(_)) {
        return Recheck::Inconcluso(format!("el minificado sin mangle no termino ({:?})", b0.status));
    }
    if !matches!(oracle::compare(a, &b0), oracle::Verdict::Same) || !mensaje_con_nombre_localizado(&b0, b) {
        return Recheck::NoExplica;
    }
    Recheck::Explica
}

/// 8.3: forma canonica de un programa modulo los nombres de sus bindings (`alfa_equivalentes`).
/// Despues del resolver de SWC, cada identificador que resuelve a un binding pasa a `l$<k>`
/// (k: el orden en que aparece por primera vez ese binding, `(sym, ctxt)`), cada uno sin
/// resolver (global) a `g$<sym>`, cada nombre privado a `p<k>` (por clase, el mas interno
/// primero) y cada nombre de import/export a `x$<sym>`. Las formas abreviadas que el mangle
/// expande (`{cb}` -> `{cb:e}`, `export{x}` -> `export{e as x}`, `import{x}` ->
/// `import{x as e}`) se expanden antes en los dos lados.
struct Canon {
    sin_resolver: swc_common::Mark,
    locales: std::collections::HashMap<(String, swc_common::SyntaxContext), usize>,
    privados: Vec<std::collections::HashMap<String, usize>>,
    n_privados: usize,
    desde_otro: bool,
    dinamico: Option<&'static str>,
    /// H1: nombre canonico (`p<k>`) -> nombre original de cada nombre privado del que sale
    /// un `.name` (metodo privado, o campo/accessor privado con una definicion anonima).
    nombran_privados: std::collections::BTreeMap<String, String>,
}

impl Canon {
    fn literal(m: &mut swc_ecma_ast::ModuleExportName) {
        if let swc_ecma_ast::ModuleExportName::Ident(i) = m {
            i.sym = format!("x${}", i.sym).into();
            i.ctxt = swc_common::SyntaxContext::empty();
        }
    }
}

impl VisitMut for Canon {
    fn visit_mut_ident(&mut self, i: &mut swc_ecma_ast::Ident) {
        i.sym = if i.ctxt.outer() == self.sin_resolver {
            format!("g${}", i.sym).into()
        } else {
            let n = self.locales.len();
            let k = *self.locales.entry((i.sym.to_string(), i.ctxt)).or_insert(n);
            format!("l${k}").into()
        };
        i.ctxt = swc_common::SyntaxContext::empty();
    }

    fn visit_mut_with_stmt(&mut self, w: &mut swc_ecma_ast::WithStmt) {
        self.dinamico.get_or_insert("`with`");
        w.visit_mut_children_with(self);
    }

    /// 9.3 (M4): `eval` directo segun la spec (EvaluateCall): el callee es una referencia a
    /// un nombre `eval` (no una propiedad) que vale %eval%. Los parentesis no cambian la
    /// referencia (`(eval)(x)`, `((eval))(x)` son directos; `(0, eval)(x)` y `eval?.(x)` no:
    /// MEDIDO en node 24.14.1, sondas de 9.3), y el nombre puede ser un binding local que vale
    /// %eval% (`function f(eval) { eval(x) }` en sloppy: tambien directo, MEDIDO). Como no se
    /// sabe que vale, cualquier llamada a un nombre `eval` cuenta (de mas: solo agrega
    /// Inconclusos).
    fn visit_mut_call_expr(&mut self, c: &mut swc_ecma_ast::CallExpr) {
        if let swc_ecma_ast::Callee::Expr(e) = &c.callee {
            let mut callee = &**e;
            while let Expr::Paren(p) = callee {
                callee = &p.expr;
            }
            if matches!(callee, Expr::Ident(i) if &*i.sym == "eval") {
                self.dinamico.get_or_insert("`eval` directo");
            }
        }
        c.visit_mut_children_with(self);
    }

    fn visit_mut_class(&mut self, c: &mut Class) {
        use swc_ecma_ast::{ClassMember, Key};
        let mut propios = std::collections::HashMap::new();
        for m in &c.body {
            // H1: el `.name` de un metodo privado es `#m` (`get #m`...), y el de una
            // definicion anonima en un campo privado tambien (NamedEvaluation).
            let anonima = |v: &Option<Box<Expr>>| v.as_deref().is_some_and(definicion_anonima);
            let (nombre, nombra) = match m {
                ClassMember::PrivateMethod(p) => (Some(&p.key.name), true),
                ClassMember::PrivateProp(p) => (Some(&p.key.name), anonima(&p.value)),
                ClassMember::AutoAccessor(p) => match &p.key {
                    Key::Private(k) => (Some(&k.name), anonima(&p.value)),
                    Key::Public(_) => (None, false),
                },
                _ => (None, false),
            };
            let Some(nombre) = nombre else { continue };
            let k = *propios.entry(nombre.to_string()).or_insert_with(|| {
                self.n_privados += 1;
                self.n_privados - 1
            });
            if nombra {
                self.nombran_privados.insert(format!("p{k}"), format!("#{nombre}"));
            }
        }
        self.privados.push(propios);
        c.visit_mut_children_with(self);
        self.privados.pop();
    }

    fn visit_mut_private_name(&mut self, p: &mut swc_ecma_ast::PrivateName) {
        let k = self.privados.iter().rev().find_map(|m| m.get(&*p.name).copied());
        p.name = match k {
            Some(k) => format!("p{k}").into(),
            None => format!("q${}", p.name).into(),
        };
    }

    fn visit_mut_named_export(&mut self, n: &mut swc_ecma_ast::NamedExport) {
        let antes = std::mem::replace(&mut self.desde_otro, n.src.is_some());
        n.visit_mut_children_with(self);
        self.desde_otro = antes;
    }

    fn visit_mut_export_named_specifier(&mut self, s: &mut swc_ecma_ast::ExportNamedSpecifier) {
        if self.desde_otro {
            Canon::literal(&mut s.orig);
        } else {
            s.orig.visit_mut_with(self);
        }
        if let Some(e) = s.exported.as_mut() {
            Canon::literal(e);
        }
    }

    fn visit_mut_export_namespace_specifier(&mut self, s: &mut swc_ecma_ast::ExportNamespaceSpecifier) {
        Canon::literal(&mut s.name);
    }

    fn visit_mut_export_default_specifier(&mut self, s: &mut swc_ecma_ast::ExportDefaultSpecifier) {
        s.exported.sym = format!("x${}", s.exported.sym).into();
        s.exported.ctxt = swc_common::SyntaxContext::empty();
    }

    fn visit_mut_import_named_specifier(&mut self, s: &mut swc_ecma_ast::ImportNamedSpecifier) {
        s.local.visit_mut_with(self);
        if let Some(i) = s.imported.as_mut() {
            Canon::literal(i);
        }
    }
}

/// 8.3: expande las formas abreviadas que el mangle cambia por la larga (ver `Canon`).
struct Expandir;

impl VisitMut for Expandir {
    fn visit_mut_prop(&mut self, p: &mut swc_ecma_ast::Prop) {
        use swc_ecma_ast::{IdentName, KeyValueProp, Prop, PropName};
        p.visit_mut_children_with(self);
        if let Prop::Shorthand(i) = p {
            let key = PropName::Ident(IdentName::new(i.sym.clone(), i.span));
            *p = Prop::KeyValue(KeyValueProp { key, value: Box::new(Expr::Ident(i.clone())) });
        }
    }

    fn visit_mut_object_pat_prop(&mut self, p: &mut swc_ecma_ast::ObjectPatProp) {
        use swc_ecma_ast::{AssignPat, IdentName, KeyValuePatProp, ObjectPatProp, Pat, PropName};
        p.visit_mut_children_with(self);
        if let ObjectPatProp::Assign(a) = p {
            let key = PropName::Ident(IdentName::new(a.key.id.sym.clone(), a.key.id.span));
            let id = Box::new(Pat::Ident(a.key.clone()));
            let value = match a.value.take() {
                Some(right) => Box::new(Pat::Assign(AssignPat { span: a.span, left: id, right })),
                None => id,
            };
            *p = ObjectPatProp::KeyValue(KeyValuePatProp { key, value });
        }
    }

    fn visit_mut_import_named_specifier(&mut self, s: &mut swc_ecma_ast::ImportNamedSpecifier) {
        if s.imported.is_none() {
            let i = swc_ecma_ast::Ident::new_no_ctxt(s.local.sym.clone(), DUMMY_SP);
            s.imported = Some(swc_ecma_ast::ModuleExportName::Ident(i));
        }
    }

    fn visit_mut_export_named_specifier(&mut self, s: &mut swc_ecma_ast::ExportNamedSpecifier) {
        if s.exported.is_none() {
            if let swc_ecma_ast::ModuleExportName::Ident(i) = &s.orig {
                let e = swc_ecma_ast::Ident::new_no_ctxt(i.sym.clone(), DUMMY_SP);
                s.exported = Some(swc_ecma_ast::ModuleExportName::Ident(e));
            }
        }
    }
}

/// H1 (8.3): los bindings `(sym, ctxt)` (despues del resolver y de `Expandir`) de los que sale
/// el `.name` de una funcion o clase, segun la spec:
/// - el nombre propio de una declaracion o expresion de funcion o de clase;
/// - el destino de NamedEvaluation de una definicion anonima (`definicion_anonima`:
///   IsAnonymousFunctionDefinition, con parentesis): `var/let/const x = ...` (y `using`,
///   que es el mismo declarador); un SingleNameBinding con inicializador (default de
///   parametro, `[x = ...]`, `{k: x = ...}`; la forma `{x = ...}` ya viene expandida); un
///   AssignmentElement o AssignmentProperty de una asignacion por destructuring (los mismos
///   `AssignPat`); y `x = ...`, `x &&= ...`, `x ||= ...`, `x ??= ...`. 9.3 (M5): con el
///   destino entre parentesis (`(x) = ...`, `[(x) = ...] = []`, `({k: (x) = ...} = {})`) no
///   hay NamedEvaluation (IsIdentifierRef es falso; MEDIDO en node: `.name` "") y no se marca.
///
/// Las claves de propiedades y de campos publicos tambien dan nombre, pero el mangle no las
/// toca; los privados los marca `Canon::visit_mut_class`. Un global no se renombra
/// (`Canon`: `g$<sym>` exige el mismo nombre).
struct Nombran(std::collections::HashSet<(String, swc_common::SyntaxContext)>);

impl Nombran {
    fn ident(&mut self, i: &swc_ecma_ast::Ident) {
        self.0.insert((i.sym.to_string(), i.ctxt));
    }

    fn patron(&mut self, p: &swc_ecma_ast::Pat, valor: &Expr) {
        if !definicion_anonima(valor) {
            return;
        }
        match p {
            swc_ecma_ast::Pat::Ident(b) => self.ident(&b.id),
            // 9.3 (M5): IsIdentifierRef de una expresion parentizada es falso (`[(h) = f] = []`
            // y `({k: (h) = f} = {})` dan `.name` "": MEDIDO en node 24.14.1): solo el nombre
            // pelado.
            swc_ecma_ast::Pat::Expr(e) => {
                if let Expr::Ident(i) = &**e {
                    self.ident(i);
                }
            }
            _ => {}
        }
    }
}

impl swc_ecma_visit::Visit for Nombran {
    fn visit_var_declarator(&mut self, d: &swc_ecma_ast::VarDeclarator) {
        if let Some(init) = &d.init {
            self.patron(&d.name, init);
        }
        swc_ecma_visit::VisitWith::visit_children_with(d, self);
    }

    fn visit_assign_pat(&mut self, p: &swc_ecma_ast::AssignPat) {
        self.patron(&p.left, &p.right);
        swc_ecma_visit::VisitWith::visit_children_with(p, self);
    }

    fn visit_assign_pat_prop(&mut self, p: &swc_ecma_ast::AssignPatProp) {
        if p.value.as_deref().is_some_and(definicion_anonima) {
            self.ident(&p.key.id);
        }
        swc_ecma_visit::VisitWith::visit_children_with(p, self);
    }

    fn visit_assign_expr(&mut self, n: &swc_ecma_ast::AssignExpr) {
        use swc_ecma_ast::{AssignOp, AssignTarget, SimpleAssignTarget};
        let op = matches!(n.op, AssignOp::Assign | AssignOp::AndAssign | AssignOp::OrAssign | AssignOp::NullishAssign);
        if op && definicion_anonima(&n.right) {
            // 9.3 (M5): `(h) = f` (y `(h) ||= f`...) no nombra: IsIdentifierRef de una
            // expresion parentizada es falso (MEDIDO en node 24.14.1: `.name` "").
            if let AssignTarget::Simple(SimpleAssignTarget::Ident(b)) = &n.left {
                self.ident(&b.id);
            }
        }
        swc_ecma_visit::VisitWith::visit_children_with(n, self);
    }

    fn visit_fn_decl(&mut self, f: &swc_ecma_ast::FnDecl) {
        self.ident(&f.ident);
        swc_ecma_visit::VisitWith::visit_children_with(f, self);
    }

    fn visit_fn_expr(&mut self, f: &swc_ecma_ast::FnExpr) {
        if let Some(i) = &f.ident {
            self.ident(i);
        }
        swc_ecma_visit::VisitWith::visit_children_with(f, self);
    }

    fn visit_class_decl(&mut self, c: &swc_ecma_ast::ClassDecl) {
        self.ident(&c.ident);
        swc_ecma_visit::VisitWith::visit_children_with(c, self);
    }

    fn visit_class_expr(&mut self, c: &swc_ecma_ast::ClassExpr) {
        if let Some(i) = &c.ident {
            self.ident(i);
        }
        swc_ecma_visit::VisitWith::visit_children_with(c, self);
    }
}

/// 8.3: la forma canonica y, H1, los nombres que dan un `.name` (nombre canonico -> nombre
/// original; `Nombran` y los privados de `Canon`).
type Canonica = (String, std::collections::BTreeMap<String, String>);

/// 8.3: la forma canonica de `src` (`Canon`). `None`: no parsea. `Err`: tiene algo que hace
/// dinamica la resolucion (`with`, `eval` directo) y la canonica no prueba nada.
fn forma_canonica(src: &str, kind: SourceKind) -> Option<Result<Canonica, String>> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
    let program = backend::swc::parse_program(&fm, kind)?;
    let (sin_resolver, top) = (swc_common::Mark::new(), swc_common::Mark::new());
    let mut program = program.apply(swc_ecma_transforms_base::resolver(sin_resolver, top, false));
    program.visit_mut_with(&mut Expandir);
    let mut nombran = Nombran(Default::default());
    swc_ecma_visit::VisitWith::visit_with(&program, &mut nombran);
    let mut c = Canon {
        sin_resolver,
        locales: Default::default(),
        privados: Vec::new(),
        n_privados: 0,
        desde_otro: false,
        dinamico: None,
        nombran_privados: Default::default(),
    };
    program.visit_mut_with(&mut c);
    if let Some(d) = c.dinamico {
        return Some(Err(format!("tiene {d}: la resolucion de nombres es dinamica")));
    }
    let mut nombres = std::mem::take(&mut c.nombran_privados);
    for clave in nombran.0 {
        if let Some(k) = c.locales.get(&clave) {
            nombres.insert(format!("l${k}"), clave.0);
        }
    }
    let mut buf = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: swc_ecma_codegen::Config::default().with_minify(true),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm, "\n", &mut buf, None),
        };
        emitter.emit_program(&program).ok()?;
    }
    String::from_utf8(buf).ok().map(|s| Ok((s, nombres)))
}

/// 8.3: ¿`b` es un renombre correcto de `b0`? (`mensaje_nombre_matters`, paso 1). Las dos
/// formas canonicas (`forma_canonica`) iguales. `None`: alguno no parsea (o un panic de SWC).
/// `Err(motivo)`: no se puede afirmar. `Ok(renombres)`: si, y H1: los bindings de los que
/// sale un `.name` (`Nombran`) que el renombre cambio, como `viejo -> nuevo` (vacio: ninguno).
pub fn alfa_equivalentes(b0: &str, b: &str, kind: SourceKind) -> Option<Result<Vec<String>, String>> {
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let c0 = forma_canonica(b0, kind)?;
            let c1 = forma_canonica(b, kind)?;
            Some(match (c0, c1) {
                (Err(m), _) | (_, Err(m)) => Err(m),
                (Ok((x, n0)), Ok((y, n1))) if x == y => {
                    // Con las formas iguales, la misma etiqueta es el mismo binding en los dos.
                    let etiquetas: std::collections::BTreeSet<&String> = n0.keys().chain(n1.keys()).collect();
                    Ok(etiquetas
                        .into_iter()
                        .filter_map(|k| {
                            let (v0, v1) = (n0.get(k), n1.get(k));
                            let s = |v: Option<&String>| v.map_or("?", |v| v.as_str()).to_string();
                            (v0 != v1).then(|| format!("{} -> {}", s(v0), s(v1)))
                        })
                        .collect())
                }
                (Ok(_), Ok(_)) => Err("las formas canonicas difieren".into()),
            })
        })
    }))
    .ok()
    .flatten()
}

/// t-principio: lo que el preload vio pisado en `a` (lineas `builtin <objeto>
/// <clave>` del canal), sin repetir.
pub fn builtins_pisados(a: &runner::Exec) -> Vec<&str> {
    let mut v: Vec<&str> = Vec::new();
    for s in a.ctl.lines().filter_map(|l| l.strip_prefix("builtin ")) {
        if !v.contains(&s) {
            v.push(s);
        }
    }
    v
}

/// t-principio: `builtins` (oxc: "No Monkey-Patching Built-ins"; antes iba a known/
/// sin re-chequeo). Como `global_conocido`, en runtime: el original y el minificado
/// corren con cada propiedad que A piso fijada a su valor original (preload,
/// DIFUZZ_BUILTINS_FIJOS: las escrituras no hacen nada). Si la divergencia sigue,
/// el pisado no la explica. Rige `veredicto`. Sin re-minificar: 2 node.
/// Paso 7 (revision-tint B1, r04/r05/r11/r15): rige `veredicto_neutralizado` (B tiene
/// que comportarse como A'): el codigo que pisa no corre en A' ni en B', y si tiene el
/// bug, B' == A' no prueba nada.
pub fn builtins_matter(r: &runner::Runner, original: &str, minified: &str, a: &runner::Exec, b: &runner::Exec) -> Recheck {
    let v = builtins_pisados(a);
    if v.is_empty() {
        return Recheck::Inconcluso("el preload no dejo ninguna linea `builtin`".into());
    }
    if v.contains(&"-") {
        return Recheck::Inconcluso("hay un built-in pisado que no se puede nombrar (Symbol propio)".into());
    }
    let spec = v.join(",");
    let env = [("DIFUZZ_BUILTINS_FIJOS", spec.as_str())];
    let a2 = r.run_with_env(original, &env);
    if let Some(l) = a2.ctl.lines().find(|l| l.starts_with("builtin_no_fijado ")) {
        return Recheck::Inconcluso(format!("no pude fijar un built-in pisado ({l})"));
    }
    if !a2.completed() {
        return Recheck::Inconcluso(format!("el original con los built-ins fijos no termino ({:?})", a2.status));
    }
    let b2 = r.run_with_env(minified, &env);
    // b7-1 (q06: el built-in pisado que B todavia llama, con el bug): B con lo pisado libre y
    // fijo, con cobertura.
    // b7-1b (revision 7.1, H2 d04): y en la corrida sin fijar, el sensor de uso de lo pisado.
    let uso = [("DIFUZZ_BUILTINS_USO", spec.as_str())];
    veredicto_neutralizado(a, b, &a2, &b2, None, || camino_entorno(r, minified, b, &b2, (&[], &env), &uso))
}

/// t-principio: `coercion` (estatico; oxc: "Coercion Methods Are Pure"; antes iba a
/// known/ sin re-chequeo). Se renombran las definiciones de helpers de coercion
/// (`neutralize_coercion`): la coercion usa la de Object.prototype, que es pura. Se
/// minifica igual y rige `veredicto`. 1 minify + 2 node.
/// Paso 7 (revision-tint B1, r01/r02/r10/r12/r14): rige `veredicto_neutralizado`: los
/// helpers renombrados no corren en A' ni en B'.
pub fn coercion_matters(
    r: &runner::Runner,
    recheck: (backend::Minifier, minify::Preset, SourceKind),
    original: &str,
    minified: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> Recheck {
    let (minifier, preset, kind) = recheck;
    let neutral = match crate::analisis::sin_coercion(original, kind) {
        Ok(n) => n,
        Err(e) => {
            crate::analisis::registrar("re-chequeo de coercion", &e);
            return Recheck::Inconcluso(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()));
        }
    };
    let Some(neutral) = neutral else {
        return Recheck::Inconcluso("no pude neutralizar los helpers de coercion (no parsea con SWC o no los encuentro)".into());
    };
    let cfg = preset.build(minifier, kind);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&neutral, &cfg) else {
        return Recheck::Inconcluso("no pude minificar la variante sin helpers de coercion".into());
    };
    let a2 = r.run(&o2);
    if !a2.completed() {
        return Recheck::Inconcluso(format!("la variante sin helpers de coercion no termino ({:?})", a2.status));
    }
    let b2 = r.run(&m2);
    // b7-1 (r07: el `valueOf` que B todavia llama, con el bug): B con cada definicion de
    // helper como clave `[__difuzzNeutroClave(...)]`, con el interruptor apagado y prendido.
    veredicto_neutralizado(a, b, &a2, &b2, None, || camino_interruptores(r, b, crate::analisis::interruptores_coercion(minified, kind)))
}

/// t-principio: el prefijo de los helpers de coercion renombrados.
pub const COERCION_RENOMBRADA: &str = "__dz_";

/// t-principio: el programa con cada DEFINICION de helper de coercion que ve
/// `checks` (`valueOf`/`toString` o `[Symbol.toPrimitive]` como clave de un
/// literal de objeto o de un miembro de clase, o asignados a una propiedad)
/// renombrada a `__dz_<nombre>`. Las lecturas (`o.valueOf()`) y los patrones de
/// desestructuracion no se tocan. `None` si no parsea, si no hay nada que
/// renombrar o si el prefijo ya aparece en el fuente.
pub fn neutralize_coercion(src: &str, kind: SourceKind) -> Option<String> {
    use swc_ecma_ast::{AssignExpr, AssignTarget, ClassMember, IdentName, MemberProp, Prop, PropName, SimpleAssignTarget};
    if src.contains(COERCION_RENOMBRADA) {
        return None;
    }
    fn nuevo(n: &str) -> IdentName {
        IdentName { span: DUMMY_SP, sym: format!("{COERCION_RENOMBRADA}{n}").into() }
    }
    fn clave(k: &mut PropName) -> bool {
        match k {
            PropName::Ident(i) if crate::checks::is_coercion_name(i.sym.as_str()) => {
                *i = nuevo(i.sym.as_str());
                true
            }
            PropName::Computed(c) if crate::checks::is_symbol_to_primitive(&c.expr) => {
                *k = PropName::Ident(nuevo("toPrimitive"));
                true
            }
            _ => false,
        }
    }
    struct N(bool);
    impl VisitMut for N {
        fn visit_mut_prop(&mut self, p: &mut Prop) {
            p.visit_mut_children_with(self);
            let k = match p {
                Prop::KeyValue(x) => &mut x.key,
                Prop::Method(x) => &mut x.key,
                Prop::Getter(x) => &mut x.key,
                Prop::Setter(x) => &mut x.key,
                _ => return,
            };
            self.0 |= clave(k);
        }
        fn visit_mut_class_member(&mut self, m: &mut ClassMember) {
            m.visit_mut_children_with(self);
            let k = match m {
                ClassMember::Method(x) => &mut x.key,
                ClassMember::ClassProp(x) => &mut x.key,
                _ => return,
            };
            self.0 |= clave(k);
        }
        fn visit_mut_assign_expr(&mut self, n: &mut AssignExpr) {
            n.visit_mut_children_with(self);
            if let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = &mut n.left {
                let nombre = match &m.prop {
                    MemberProp::Ident(i) if crate::checks::is_coercion_name(i.sym.as_str()) => Some(i.sym.to_string()),
                    MemberProp::Computed(c) if crate::checks::is_symbol_to_primitive(&c.expr) => Some("toPrimitive".into()),
                    _ => None,
                };
                if let Some(nombre) = nombre {
                    m.prop = MemberProp::Ident(nuevo(&nombre));
                    self.0 = true;
                }
            }
        }
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
            let mut program = backend::swc::parse_program(&fm, kind)?;
            let mut n = N(false);
            program.visit_mut_with(&mut n);
            if !n.0 {
                return None;
            }
            emit(cm, &program.apply(fixer(None)))
        })
    }))
    .ok()
    .flatten()
}

/// b4-a: los flags del sensor de violaciones (preload.js) que re-chequea
/// `violaciones_matter`, cada uno con la clase de su linea `viol`.
/// I2 (revision-b456): `tdz` (la no atrapada) tambien: antes iba a known/ sin
/// re-chequeo (r09-tdz-final-getter: un bug ajeno y `x; let x;` al final). El
/// `extends` no atrapado no esta aca: es el flag `extends`, que ya re-chequea
/// `Rechequeo::Extends` (clasificar::extends_matters).
pub const FLAGS_VIOLACION: [(&str, &str); 4] =
    [("tdz_atrapada", "tdz"), ("coercion_symbol", "symbol"), ("extends_atrapado", "extends"), ("tdz", "tdz")];

/// b4-a: las violaciones que el preload ubico en `a` (lineas `viol <clase> <pos>`
/// del canal de control), sin repetir, en el orden en que pasaron.
pub fn violaciones(a: &runner::Exec) -> Vec<&str> {
    let mut v: Vec<&str> = Vec::new();
    for s in a.ctl.lines().filter_map(|l| l.strip_prefix("viol ")) {
        if !v.contains(&s) {
            v.push(s);
        }
    }
    v
}

/// b4-a (R1-H04 / C3): `Explica` solo si, con cada violacion ubicada en A
/// cambiada por algo que no lanza (`neutralize_violaciones`), la divergencia
/// desaparece y el camino se sostiene (`veredicto`). Se neutralizan todas las
/// ubicadas, de las tres clases a la vez: con una sola, la variante podria seguir
/// divergiendo por otra y el veredicto dependeria del orden.
/// n-principio (n06/n07/n10/n11/n12/n17/n18: el bug en el catch de la violacion, que en
/// A' no corre): rige `veredicto_neutralizado`, B tiene que dar lo mismo que A'.
/// `i3` (integracion 7.1 + 7.4): el runner con el sensor si A y B se compararon sin el
/// sensor (`camino_de_b`). 8.7: `minified`, el texto de B (para su camino).
#[allow(clippy::too_many_arguments)]
pub fn violaciones_matter(
    r: &runner::Runner,
    recheck: (backend::Minifier, minify::Preset, SourceKind),
    original: &str,
    minified: &str,
    a: &runner::Exec,
    b: &runner::Exec,
    i3: Option<&runner::Runner>,
) -> Recheck {
    let (minifier, preset, kind) = recheck;
    let v = violaciones(a);
    if v.is_empty() {
        return Recheck::Inconcluso("el preload no dejo ninguna linea `viol` (tope de pausas o sin inspector)".into());
    }
    if let Some(s) = v.iter().find(|s| s.ends_with(" -")) {
        return Recheck::Inconcluso(format!("violacion sin posicion: {s}"));
    }
    let spec = v.join("\n");
    let neutral = match crate::analisis::sin_violaciones(original, kind, &spec) {
        Ok(n) => n,
        Err(e) => {
            crate::analisis::registrar("re-chequeo de violaciones atrapadas", &e);
            return Recheck::Inconcluso(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()));
        }
    };
    let Some(neutral) = neutral else {
        return Recheck::Inconcluso(format!("no pude ubicar en el original todas las violaciones ({})", v.join(", ")));
    };
    let cfg = preset.build(minifier, kind);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&neutral, &cfg) else {
        return Recheck::Inconcluso("no pude minificar la variante sin violaciones".into());
    };
    let a2 = r.run(&o2);
    if !a2.completed() {
        return Recheck::Inconcluso(format!("la variante sin violaciones no termino ({:?})", a2.status));
    }
    let b2 = r.run(&m2);
    // b7-1 (r01/r03/r09: la TDZ o la coercion de Symbol que B todavia hace, con el bug en el
    // catch): el sensor no tiene que ver en B ninguna violacion.
    veredicto_neutralizado(a, b, &a2, &b2, None, || camino_de_b(r, i3, minified, b, None))
}

/// b4-a: el programa con cada violacion de `spec` (`<clase> <linea>:<columna>`,
/// 0-based y columna en UTF-16, como las da V8) cambiada por algo que no lanza:
/// - `tdz`: el identificador que empieza ahi pasa a `void 0`;
/// - `symbol`: la expresion mas chica que la contiene y que puede coercionar
///   (llamada, `new`, binaria, unaria, template, update, asignacion) pasa a `void 0`;
/// - `extends`: el heredero que la contiene pasa a `null`.
///
/// `None` si no parsea o si alguna violacion no se ubica (V8 da 0:0 para un
/// heredero literal, p. ej.): un re-chequeo a medias no es concluyente.
///
/// n-principio (MEJORAS, Fase R I2): V8 ubica una TDZ dentro de `return`/`throw` en la
/// SENTENCIA, no en el identificador (`function g() { return x; }` da la columna de
/// `return`). Si una `tdz` no cae en un identificador, se busca un `return`/`throw` que
/// empiece justo ahi y se cuentan sus candidatos: identificadores de su argumento cuyo
/// nombre esta ligado con `let`/`const`/`class` en alguna parte del programa (los unicos
/// que tienen TDZ), sin entrar en funciones, flechas ni clases anidadas (su cuerpo no
/// corre ahi). Con exactamente uno, ese identificador (solo el) pasa a `void 0`; el
/// resto de la sentencia queda, asi que un bug en la misma sentencia sigue en A' y B'.
/// Con cero o mas de uno (`return a + b`, o `x` dos veces), no se ubica: `None`.
/// Un candidato equivocado no manda a known/ un bug: `veredicto_neutralizado` exige
/// B == A' (si el identificador cambiado no era el de la TDZ, A' sigue lanzando como A).
pub fn neutralize_violaciones(src: &str, kind: SourceKind, spec: &str) -> Option<String> {
    use std::collections::HashSet;
    use swc_ecma_ast::{ArrowExpr, ClassDecl, Function, VarDecl, VarDeclKind};
    use swc_ecma_visit::{Visit, VisitWith};
    /// n-principio: los nombres ligados con `let`/`const`/`class` en cualquier scope.
    struct Lexicos(HashSet<String>);
    impl Visit for Lexicos {
        fn visit_var_decl(&mut self, v: &VarDecl) {
            if v.kind != VarDeclKind::Var {
                struct Ligados<'a>(&'a mut HashSet<String>);
                impl Visit for Ligados<'_> {
                    fn visit_binding_ident(&mut self, b: &swc_ecma_ast::BindingIdent) {
                        self.0.insert(b.id.sym.to_string());
                    }
                    fn visit_assign_pat_prop(&mut self, p: &swc_ecma_ast::AssignPatProp) {
                        self.0.insert(p.key.id.sym.to_string());
                    }
                    fn visit_expr(&mut self, _: &Expr) {}
                }
                for d in &v.decls {
                    d.name.visit_with(&mut Ligados(&mut self.0));
                }
            }
            v.visit_children_with(self);
        }
        fn visit_class_decl(&mut self, c: &ClassDecl) {
            self.0.insert(c.ident.sym.to_string());
            c.visit_children_with(self);
        }
    }
    /// n-principio: cuenta los candidatos de un argumento (sin entrar en lo anidado).
    struct Candidatos<'a>(&'a HashSet<String>, usize);
    impl Visit for Candidatos<'_> {
        fn visit_function(&mut self, _: &Function) {}
        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
        fn visit_class(&mut self, _: &Class) {}
        fn visit_expr(&mut self, e: &Expr) {
            if let Expr::Ident(i) = e {
                self.1 += usize::from(self.0.contains(&*i.sym));
            }
            e.visit_children_with(self);
        }
    }
    /// n-principio: cambia el (unico) candidato por `void 0`.
    struct Cambia<'a>(&'a HashSet<String>);
    impl VisitMut for Cambia<'_> {
        fn visit_mut_function(&mut self, _: &mut Function) {}
        fn visit_mut_arrow_expr(&mut self, _: &mut ArrowExpr) {}
        fn visit_mut_class(&mut self, _: &mut Class) {}
        fn visit_mut_expr(&mut self, e: &mut Expr) {
            if matches!(e, Expr::Ident(i) if self.0.contains(&*i.sym)) {
                *e = *Expr::undefined(DUMMY_SP);
                return;
            }
            e.visit_mut_children_with(self);
        }
    }
    /// n-principio: las `tdz` que no cayeron en un identificador, por sentencia.
    struct Sentencia<'a>(&'a HashSet<String>, Vec<(BytePos, bool)>);
    impl VisitMut for Sentencia<'_> {
        fn visit_mut_stmt(&mut self, s: &mut Stmt) {
            s.visit_mut_children_with(self);
            let (lo, arg) = match s {
                Stmt::Return(r) => (r.span.lo, r.arg.as_deref_mut()),
                Stmt::Throw(t) => (t.span.lo, Some(&mut *t.arg)),
                _ => return,
            };
            let Some(arg) = arg else { return };
            if !self.1.iter().any(|p| !p.1 && p.0 == lo) {
                return;
            }
            let mut c = Candidatos(self.0, 0);
            arg.visit_with(&mut c);
            if c.1 != 1 {
                return;
            }
            arg.visit_mut_with(&mut Cambia(self.0));
            for p in self.1.iter_mut().filter(|p| p.0 == lo) {
                p.1 = true;
            }
        }
    }
    #[derive(PartialEq)]
    enum Clase {
        Tdz,
        Symbol,
        Extends,
    }
    struct N(Vec<(Clase, BytePos, bool)>);
    impl VisitMut for N {
        fn visit_mut_class(&mut self, c: &mut Class) {
            c.visit_mut_children_with(self);
            if let Some(sup) = c.super_class.as_mut() {
                let sp = sup.span();
                let mut hit = false;
                for o in self.0.iter_mut().filter(|o| o.0 == Clase::Extends && !o.2 && sp.lo <= o.1 && o.1 < sp.hi) {
                    o.2 = true;
                    hit = true;
                }
                if hit {
                    **sup = Expr::Lit(Lit::Null(Null { span: DUMMY_SP }));
                }
            }
        }
        fn visit_mut_expr(&mut self, e: &mut Expr) {
            e.visit_mut_children_with(self);
            let sp = e.span();
            let ident = matches!(e, Expr::Ident(_));
            let coerce = matches!(
                e,
                Expr::Call(_) | Expr::New(_) | Expr::Bin(_) | Expr::Unary(_) | Expr::Tpl(_) | Expr::Update(_) | Expr::Assign(_)
            );
            let mut hit = false;
            for o in self.0.iter_mut().filter(|o| !o.2) {
                let si = match o.0 {
                    Clase::Tdz => ident && sp.lo == o.1,
                    Clase::Symbol => coerce && sp.lo <= o.1 && o.1 < sp.hi,
                    Clase::Extends => false,
                };
                if si {
                    o.2 = true;
                    hit = true;
                }
            }
            if hit {
                *e = *Expr::undefined(DUMMY_SP);
            }
        }
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
            let mut objetivos = Vec::new();
            for l in spec.lines() {
                let (clase, pos) = l.split_once(' ')?;
                let (lin, col) = pos.split_once(':')?;
                let off = offset_utf16(src, lin.parse().ok()?, col.parse().ok()?)?;
                let clase = match clase {
                    "tdz" => Clase::Tdz,
                    "symbol" => Clase::Symbol,
                    "extends" => Clase::Extends,
                    _ => return None,
                };
                objetivos.push((clase, fm.start_pos + BytePos(u32::try_from(off).ok()?), false));
            }
            if objetivos.is_empty() {
                return None;
            }
            let mut program = backend::swc::parse_program(&fm, kind)?;
            let mut n = N(objetivos);
            program.visit_mut_with(&mut n);
            // n-principio: las `tdz` que quedaron, en la sentencia (`return`/`throw`).
            let pendientes: Vec<(BytePos, bool)> = n.0.iter().filter(|o| !o.2 && o.0 == Clase::Tdz).map(|o| (o.1, false)).collect();
            if !pendientes.is_empty() && n.0.iter().all(|o| o.2 || o.0 == Clase::Tdz) {
                let mut lex = Lexicos(HashSet::new());
                program.visit_with(&mut lex);
                let mut s = Sentencia(&lex.0, pendientes);
                program.visit_mut_with(&mut s);
                for o in n.0.iter_mut().filter(|o| !o.2) {
                    o.2 = s.1.iter().any(|p| p.1 && p.0 == o.1);
                }
            }
            if !n.0.iter().all(|o| o.2) {
                return None;
            }
            emit(cm, &program.apply(fixer(None)))
        })
    }))
    .ok()
    .flatten()
}

/// b4-a: el byte de `src` en la linea `linea` y la columna `col` (0-based, en
/// unidades UTF-16, con los terminadores de linea de ECMAScript, como V8).
fn offset_utf16(src: &str, linea: usize, col: usize) -> Option<usize> {
    let mut inicio = 0;
    let mut l = 0;
    let mut it = src.char_indices().peekable();
    while l < linea {
        let (i, c) = it.next()?;
        match c {
            '\n' | '\u{2028}' | '\u{2029}' => {
                l += 1;
                inicio = i + c.len_utf8();
            }
            '\r' => {
                l += 1;
                inicio = i + 1;
                if let Some(&(j, '\n')) = it.peek() {
                    it.next();
                    inicio = j + 1;
                }
            }
            _ => {}
        }
    }
    let mut u = 0;
    for (i, c) in src[inicio..].char_indices() {
        if u == col {
            return Some(inicio + i);
        }
        if u > col || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            return None;
        }
        u += c.len_utf16();
    }
    (u == col).then_some(src.len())
}

/// El programa con cada `with (o) S` cambiado por `{ o; S }`. `None` si no
/// parsea o no hay ningun `with`.
pub fn neutralize_with(src: &str, kind: SourceKind) -> Option<String> {
    struct N(bool);
    impl VisitMut for N {
        fn visit_mut_stmt(&mut self, s: &mut Stmt) {
            s.visit_mut_children_with(self);
            if let Stmt::With(w) = s {
                let obj = std::mem::take(&mut w.obj);
                let body = std::mem::take(&mut *w.body);
                *s = Stmt::Block(BlockStmt {
                    stmts: vec![Stmt::Expr(ExprStmt { span: DUMMY_SP, expr: obj }), body],
                    ..Default::default()
                });
                self.0 = true;
            }
        }
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
            let mut program = backend::swc::parse_program(&fm, kind)?;
            let mut n = N(false);
            program.visit_mut_with(&mut n);
            if !n.0 {
                return None;
            }
            emit(cm, &program.apply(fixer(None)))
        })
    }))
    .ok()
    .flatten()
}

/// b7-1: los interruptores que define el preload con DIFUZZ_NEUTRO (`camino_interruptores`).
pub const INTERRUPTOR_WITH: &str = "__difuzzNeutroWith";
pub const INTERRUPTOR_CLAVE: &str = "__difuzzNeutroClave";
/// b7-1b (revision 7.1, H1/H2): los ganchos que define el preload con DIFUZZ_COBERTURA=1:
/// el contador de rama, el uso de un helper de coercion (al principio de su cuerpo, o
/// envolviendo su valor) y el `this` de una funcion sloppy (`__difuzzNeutroThis`, dentro de
/// `THIS_INSERTO`). b7-1c: con DIFUZZ_GANCHOS=1 (`GANCHOS`), no con toda corrida con cobertura.
pub const GANCHO_RAMA: &str = "__difuzzNeutroRama";
pub const GANCHO_USO: &str = "__difuzzNeutroUso";
pub const GANCHO_VALOR: &str = "__difuzzNeutroValor";

/// b7-1: `callee(arg)` con `callee` un identificador libre.
fn llamar(callee: &str, arg: Expr) -> Box<Expr> {
    llamar_con(callee, vec![arg])
}

/// b7-1b: `callee(args...)` con `callee` un identificador libre.
fn llamar_con(callee: &str, args: Vec<Expr>) -> Box<Expr> {
    use swc_ecma_ast::{CallExpr, Callee, ExprOrSpread, Ident};
    Box::new(Expr::Call(CallExpr {
        span: DUMMY_SP,
        callee: Callee::Expr(Box::new(Expr::Ident(Ident::new_no_ctxt(callee.into(), DUMMY_SP)))),
        args: args.into_iter().map(|a| ExprOrSpread { spread: None, expr: Box::new(a) }).collect(),
        ..Default::default()
    }))
}

/// b7-1b: un literal de cadena.
fn cadena(s: &str) -> Expr {
    Expr::Lit(Lit::Str(swc_ecma_ast::Str { span: DUMMY_SP, value: s.into(), raw: None }))
}

/// b7-1b: una sentencia de expresion.
fn sentencia(e: Box<Expr>) -> Stmt {
    Stmt::Expr(ExprStmt { span: DUMMY_SP, expr: e })
}

/// b7-1b: una directiva (`"use strict";` y cualquier otra cadena suelta al principio).
fn es_directiva(s: &Stmt) -> bool {
    matches!(s, Stmt::Expr(e) if matches!(&*e.expr, Expr::Lit(Lit::Str(_))))
}

/// b7-1b: si el prologo de directivas tiene `"use strict"`.
fn tiene_use_strict(stmts: &[Stmt]) -> bool {
    stmts
        .iter()
        .take_while(|s| es_directiva(s))
        .any(|s| matches!(s, Stmt::Expr(e) if matches!(&*e.expr, Expr::Lit(Lit::Str(x)) if x.value.to_string_lossy() == "use strict")))
}

/// b7-1b: `s` como primera sentencia del cuerpo, despues de las directivas (una sentencia antes
/// de `"use strict"` lo convertiria en una expresion suelta).
fn al_principio(stmts: &mut Vec<Stmt>, s: Stmt) {
    let i = stmts.iter().take_while(|x| es_directiva(x)).count();
    stmts.insert(i, s);
}

/// b7-1b: una funcion, flecha o clase sin nombre (con o sin parentesis). Asignada a un nombre
/// (o como valor por defecto) toma ese nombre (NamedEvaluation): envolverla en otra expresion
/// le cambiaria el `.name`. Evaluarla no corre codigo (salvo las claves computadas de una
/// clase).
fn definicion_anonima(e: &Expr) -> bool {
    match e {
        Expr::Paren(p) => definicion_anonima(&p.expr),
        Expr::Fn(f) => f.ident.is_none(),
        Expr::Arrow(_) => true,
        Expr::Class(c) => c.ident.is_none(),
        _ => false,
    }
}

/// b7-1: parsea, aplica `f` y emite. `f` dice si cambio algo: si no, `Some(None)` (no hay nada
/// que cambiar). `None`: no se pudo (no parsea, no emite o el fuente ya nombra los
/// interruptores o los ganchos): eso no es "nada que cambiar", es no saber.
fn instrumentar(src: &str, kind: SourceKind, f: impl FnOnce(&mut Program) -> bool) -> Option<Option<String>> {
    if src.contains("__difuzzNeutro") {
        return None;
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
            let mut program = backend::swc::parse_program(&fm, kind)?;
            if !f(&mut program) {
                return Some(None);
            }
            emit(cm, &program.apply(fixer(None))).map(Some)
        })
    }))
    .ok()
    .flatten()
}

/// b7-1b (revision 7.1, H1): pone un contador (`(__difuzzNeutroRama(<i>), x)`) delante de cada
/// parte condicional que la cobertura de bloques de V8 no separa: el lado derecho de `&&=`,
/// `||=` y `??=` (cualquier destino) y el valor por defecto de un parametro o de una
/// desestructuracion (en declaraciones, asignaciones, `for`, `catch`). MEDIDO (node 24.14.1,
/// cobertura precisa con `callCount` y `detailed`, `proto/p4.cjs`): con y sin esa parte, los
/// rangos son los mismos; `?.`, `&&`, `||`, `??`, el ternario, `if`, `switch`, los bucles,
/// `try` y `break` si se separan. Sin contador: una definicion anonima (`definicion_anonima`).
/// Devuelve si puso alguno.
fn ramas(program: &mut Program) -> bool {
    use swc_ecma_ast::{AssignExpr, AssignOp, AssignPat, AssignPatProp, Number, SeqExpr};
    struct R(u32);
    impl R {
        fn contar(&mut self, e: &mut Box<Expr>) {
            if definicion_anonima(e) {
                return;
            }
            let x = std::mem::take(e);
            let i = Expr::Lit(Lit::Num(Number { span: DUMMY_SP, value: f64::from(self.0), raw: None }));
            **e = Expr::Seq(SeqExpr { span: DUMMY_SP, exprs: vec![llamar(GANCHO_RAMA, i), x] });
            self.0 += 1;
        }
    }
    impl VisitMut for R {
        fn visit_mut_assign_expr(&mut self, n: &mut AssignExpr) {
            n.visit_mut_children_with(self);
            if matches!(n.op, AssignOp::AndAssign | AssignOp::OrAssign | AssignOp::NullishAssign) {
                self.contar(&mut n.right);
            }
        }
        fn visit_mut_assign_pat(&mut self, p: &mut AssignPat) {
            p.visit_mut_children_with(self);
            self.contar(&mut p.right);
        }
        fn visit_mut_assign_pat_prop(&mut self, p: &mut AssignPatProp) {
            p.visit_mut_children_with(self);
            if let Some(v) = &mut p.value {
                self.contar(v);
            }
        }
    }
    let mut r = R(0);
    program.visit_mut_with(&mut r);
    r.0 > 0
}

/// b7-1c (revision 7.1b, P1): como `instrumentar`, pero sin re-emitir: `f` junta inserciones
/// (posicion en bytes del fuente, texto) y se insertan en el texto de B tal cual. Asi el texto de
/// cada funcion de B sigue siendo el de B mas lo insertado, y el preload (con DIFUZZ_GANCHOS=1,
/// `quitarGanchos`) lo saca en `Function.prototype.toString`: un programa que lee el texto de
/// sus funciones ve el de B (con el re-emitido de SWC veia otro texto entero, y con un contador
/// `(__difuzzNeutroRama(0), 1)`: `p1-tostring-default`). Cada insercion empieza o termina en
/// `GANCHOS_TEXTO` y nada mas en el fuente lo nombra (`instrumentar`), asi que lo que el preload
/// saca es exactamente lo insertado. El resultado se vuelve a parsear: si no parsea, `None`.
/// Orden en una misma posicion: primero los cierres (el de adentro antes), despues las aperturas
/// (la de afuera antes); `f` las junta en post-orden (`seq` creciente: la de adentro primero).
fn insertar(src: &str, kind: SourceKind, f: impl FnOnce(&Program) -> Vec<Insercion>) -> Option<Option<String>> {
    if src.contains("__difuzzNeutro") {
        return None;
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
            let program = backend::swc::parse_program(&fm, kind)?;
            let mut ins = f(&program);
            if ins.is_empty() {
                return Some(None);
            }
            let base = fm.start_pos.0;
            let mut pos: Vec<(usize, u8, i64, String)> = Vec::with_capacity(ins.len());
            for (seq, i) in ins.drain(..).enumerate() {
                let p = usize::try_from(i.pos.0.checked_sub(base)?).ok()?;
                let seq = i64::try_from(seq).ok()?;
                pos.push(if i.cierre { (p, 0, seq, i.texto) } else { (p, 1, -seq, i.texto) });
            }
            pos.sort_by_key(|x| (x.0, x.1, x.2));
            let mut out = String::with_capacity(src.len() + pos.iter().map(|x| x.3.len()).sum::<usize>());
            let mut ultimo = 0;
            for (p, _, _, t) in pos {
                if p < ultimo || p > src.len() || !src.is_char_boundary(p) {
                    return None;
                }
                out.push_str(&src[ultimo..p]);
                out.push_str(&t);
                ultimo = p;
            }
            out.push_str(&src[ultimo..]);
            let fm2 = cm.new_source_file(FileName::Anon.into(), out.clone());
            backend::swc::parse_program(&fm2, kind)?;
            Some(Some(out))
        })
    }))
    .ok()
    .flatten()
}

/// b7-1c: una insercion de `insertar`: en `pos` (del fuente parseado), `texto`; `cierre` si cierra
/// algo que abrio otra insercion.
struct Insercion {
    pos: BytePos,
    cierre: bool,
    texto: String,
}

/// b7-1c (revision 7.1b, P1): lo que insertan `instrumentar_ramas` e `interruptores_sloppy` y saca el
/// preload (`quitarGanchos`, preload.js). La apertura de un contador es `(__difuzzNeutroRama(<i>),`,
/// el cierre `RAMA_FIN` y el gancho del `this` `THIS_INSERTO`. Si se cambian, cambiar el preload.
pub const RAMA_FIN: &str = " /*__difuzzNeutroFin*/)";
pub const THIS_INSERTO: &str = ";__difuzzNeutroThis(this);";

/// b7-1c: lo mismo que `ramas` (las mismas partes, el mismo orden de los indices), como inserciones
/// en el texto (`insertar`): `(__difuzzNeutroRama(<i>), x /*__difuzzNeutroFin*/)`.
fn ramas_texto(program: &Program, ins: &mut Vec<Insercion>) {
    use swc_ecma_ast::{AssignExpr, AssignOp, AssignPat, AssignPatProp};
    use swc_ecma_visit::{Visit, VisitWith};
    struct R<'a>(u32, &'a mut Vec<Insercion>);
    impl R<'_> {
        fn contar(&mut self, e: &Expr) {
            if definicion_anonima(e) {
                return;
            }
            let s = e.span();
            // Post-orden: el cierre de adentro se anota antes que el de afuera.
            self.1.push(Insercion { pos: s.hi, cierre: true, texto: RAMA_FIN.into() });
            self.1.push(Insercion { pos: s.lo, cierre: false, texto: format!("({GANCHO_RAMA}({}),", self.0) });
            self.0 += 1;
        }
    }
    impl Visit for R<'_> {
        fn visit_assign_expr(&mut self, n: &AssignExpr) {
            n.visit_children_with(self);
            if matches!(n.op, AssignOp::AndAssign | AssignOp::OrAssign | AssignOp::NullishAssign) {
                self.contar(&n.right);
            }
        }
        fn visit_assign_pat(&mut self, p: &AssignPat) {
            p.visit_children_with(self);
            self.contar(&p.right);
        }
        fn visit_assign_pat_prop(&mut self, p: &AssignPatProp) {
            p.visit_children_with(self);
            if let Some(v) = &p.value {
                self.contar(v);
            }
        }
    }
    program.visit_with(&mut R(0, ins));
}

/// b7-1b (revision 7.1, H1): el minificado con los contadores de rama (`ramas`), para el camino
/// de B por cobertura (`camino_entorno`). `Some(None)` si no hay donde ponerlos (B corre tal
/// cual); `None` si no se pudo (`instrumentar`).
/// b7-1c (revision 7.1b, P1): por insercion en el texto de B (`insertar`, `ramas_texto`), no
/// re-emitido: el texto de las funciones no cambia para el programa.
pub fn instrumentar_ramas(src: &str, kind: SourceKind) -> Option<Option<String>> {
    insertar(src, kind, |p| {
        let mut ins = Vec::new();
        ramas_texto(p, &mut ins);
        ins
    })
}

/// b7-1: el minificado con cada `with (o) S` como `with (__difuzzNeutroWith(o)) S`. Con
/// DIFUZZ_NEUTRO=0 el interruptor devuelve `o` (B tal cual); con 1, un objeto sin
/// propiedades: cada nombre de `S` resuelve afuera, como en `{ o; S; }` (`neutralize_with`).
/// `Some(None)` si no hay `with`; `None` si no se pudo (`instrumentar`).
/// b7-1b (revision 7.1, H1/H2 d02b): con DIFUZZ_NEUTRO=0 y cobertura, el interruptor devuelve
/// un Proxy de `o` que anota cada nombre que resuelve en el (el sensor de uso, preload.js); y
/// el minificado lleva ademas los contadores de rama (`ramas`). `Some(None)` sigue siendo "no
/// hay `with`" aunque haya donde poner contadores: sin `with`, B no depende de el.
pub fn interruptores_with(src: &str, kind: SourceKind) -> Option<Option<String>> {
    struct N(bool);
    impl VisitMut for N {
        fn visit_mut_stmt(&mut self, s: &mut Stmt) {
            s.visit_mut_children_with(self);
            if let Stmt::With(w) = s {
                let obj = std::mem::take(&mut w.obj);
                w.obj = llamar(INTERRUPTOR_WITH, *obj);
                self.0 = true;
            }
        }
    }
    instrumentar(src, kind, |p| {
        let mut n = N(false);
        p.visit_mut_with(&mut n);
        if n.0 {
            ramas(p);
        }
        n.0
    })
}

/// b7-1: el minificado con cada definicion de helper de coercion que renombra
/// `neutralize_coercion` (las mismas claves) como clave computada
/// `[__difuzzNeutroClave("valueOf")]` (o `[__difuzzNeutroClave(Symbol.toPrimitive)]`).
/// Con DIFUZZ_NEUTRO=0 el interruptor devuelve la clave (B tal cual: un metodo con clave
/// computada se llama igual); con 1, `__dz_<nombre>`, como el renombre del original.
/// `Some(None)` si no hay nada que cambiar; `None` si no se pudo (`instrumentar`).
/// b7-1b (revision 7.1, H2 d03): ademas, el sensor de uso: cada helper definido como funcion
/// (metodo, getter, setter, funcion o flecha como valor) empieza con
/// `__difuzzNeutroUso("<nombre>")`, y cualquier otro valor va envuelto en
/// `__difuzzNeutroValor(v, "<nombre>")` (si es una funcion, tambien nativa como
/// `Array.prototype.join`, el preload la cambia por una que anota la llamada). Y los contadores
/// de rama (`ramas`).
pub fn interruptores_coercion(src: &str, kind: SourceKind) -> Option<Option<String>> {
    use swc_ecma_ast::{AssignExpr, AssignTarget, ClassMember, ComputedPropName, Function, MemberProp, Prop, PropName, SimpleAssignTarget};
    fn clave_de(k: &PropName) -> Option<(Expr, &'static str)> {
        match k {
            PropName::Ident(i) if crate::checks::is_coercion_name(i.sym.as_str()) => {
                Some((cadena(i.sym.as_str()), if i.sym.as_str() == "valueOf" { "valueOf" } else { "toString" }))
            }
            PropName::Computed(c) if crate::checks::is_symbol_to_primitive(&c.expr) => Some(((*c.expr).clone(), "toPrimitive")),
            _ => None,
        }
    }
    fn clave(k: &mut PropName) -> Option<&'static str> {
        let (e, nombre) = clave_de(k)?;
        *k = PropName::Computed(ComputedPropName { span: DUMMY_SP, expr: llamar(INTERRUPTOR_CLAVE, e) });
        Some(nombre)
    }
    fn uso_funcion(f: &mut Function, nombre: &str) {
        if let Some(b) = &mut f.body {
            al_principio(&mut b.stmts, sentencia(llamar(GANCHO_USO, cadena(nombre))));
        }
    }
    fn uso_valor(e: &mut Box<Expr>, nombre: &str) {
        use swc_ecma_ast::{ArrowFunctionBody, SeqExpr};
        match &mut **e {
            Expr::Paren(p) => uso_valor(&mut p.expr, nombre),
            Expr::Fn(f) => uso_funcion(&mut f.function, nombre),
            Expr::Arrow(a) => match &mut *a.body {
                ArrowFunctionBody::FunctionBody(b) => al_principio(&mut b.stmts, sentencia(llamar(GANCHO_USO, cadena(nombre)))),
                ArrowFunctionBody::Expr(x) => {
                    let v = std::mem::take(x);
                    **x = Expr::Seq(SeqExpr { span: DUMMY_SP, exprs: vec![llamar(GANCHO_USO, cadena(nombre)), v] });
                }
            },
            _ => {
                let v = std::mem::take(e);
                *e = llamar_con(GANCHO_VALOR, vec![*v, cadena(nombre)]);
            }
        }
    }
    struct N(bool);
    impl VisitMut for N {
        fn visit_mut_prop(&mut self, p: &mut Prop) {
            p.visit_mut_children_with(self);
            let Some(nombre) = (match p {
                Prop::KeyValue(x) => clave(&mut x.key),
                Prop::Method(x) => clave(&mut x.key),
                Prop::Getter(x) => clave(&mut x.key),
                Prop::Setter(x) => clave(&mut x.key),
                _ => None,
            }) else {
                return;
            };
            self.0 = true;
            match p {
                Prop::KeyValue(x) => uso_valor(&mut x.value, nombre),
                Prop::Method(x) => uso_funcion(&mut x.function, nombre),
                Prop::Getter(x) => uso_funcion(&mut x.function, nombre),
                Prop::Setter(x) => uso_funcion(&mut x.function, nombre),
                _ => {}
            }
        }
        fn visit_mut_class_member(&mut self, m: &mut ClassMember) {
            m.visit_mut_children_with(self);
            match m {
                ClassMember::Method(x) => {
                    if let Some(nombre) = clave(&mut x.key) {
                        self.0 = true;
                        uso_funcion(&mut x.function, nombre);
                    }
                }
                ClassMember::ClassProp(x) => {
                    if let Some(nombre) = clave(&mut x.key) {
                        self.0 = true;
                        if let Some(v) = &mut x.value {
                            uso_valor(v, nombre);
                        }
                    }
                }
                _ => {}
            }
        }
        fn visit_mut_assign_expr(&mut self, n: &mut AssignExpr) {
            n.visit_mut_children_with(self);
            if let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = &mut n.left {
                let e = match &m.prop {
                    MemberProp::Ident(i) if crate::checks::is_coercion_name(i.sym.as_str()) => {
                        Some((cadena(i.sym.as_str()), if i.sym.as_str() == "valueOf" { "valueOf" } else { "toString" }))
                    }
                    MemberProp::Computed(c) if crate::checks::is_symbol_to_primitive(&c.expr) => Some(((*c.expr).clone(), "toPrimitive")),
                    _ => None,
                };
                if let Some((e, nombre)) = e {
                    m.prop = MemberProp::Computed(ComputedPropName { span: DUMMY_SP, expr: llamar(INTERRUPTOR_CLAVE, e) });
                    uso_valor(&mut n.right, nombre);
                    self.0 = true;
                }
            }
        }
    }
    instrumentar(src, kind, |p| {
        let mut n = N(false);
        p.visit_mut_with(&mut n);
        if n.0 {
            ramas(p);
        }
        n.0
    })
}

/// b7-1b (revision 7.1, H2 d07): el minificado para el camino de solo sloppy
/// (`camino_estricto`): cada funcion sloppy (no dentro de una clase ni de algo con
/// `"use strict"`) que usa su propio `this` (el de una flecha adentro tambien) empieza con
/// `__difuzzNeutroThis(this)` (despues de las directivas). En la corrida en estricto, un `this`
/// que sloppy hubiera cambiado (undefined, null, un primitivo) es uso de la asuncion: B elige
/// un valor por eso aunque no cambie de camino (`[a, b][+(f() !== void 0)]`). Y los contadores
/// de rama (`ramas`). Lo que no ve: el `arguments` atado a los parametros y los demas cambios de
/// sloppy que no lanzan en estricto (el camino de la cobertura sigue mirando). `Some(None)` si
/// no hay nada que poner; `None` si no se pudo (`instrumentar`). Carril A (script).
pub fn interruptores_sloppy(src: &str) -> Option<Option<String>> {
    use swc_ecma_ast::{Function, ThisExpr};
    use swc_ecma_visit::{Visit, VisitWith};
    fn usa_this(f: &Function) -> bool {
        struct B(bool);
        impl Visit for B {
            fn visit_this_expr(&mut self, _: &ThisExpr) {
                self.0 = true;
            }
            // Otra funcion u otra clase: otro `this`.
            fn visit_function(&mut self, _: &Function) {}
            fn visit_class(&mut self, _: &Class) {}
        }
        let mut b = B(false);
        f.params.visit_with(&mut b);
        f.body.visit_with(&mut b);
        b.0
    }
    // b7-1c (revision 7.1b, P1): por insercion en el texto (`insertar`): `;__difuzzNeutroThis(this);`
    // despues de las directivas (el `;` de adelante: una directiva sin `;` no se pega a la llamada).
    struct S<'a> {
        estricto: bool,
        ins: &'a mut Vec<Insercion>,
    }
    impl Visit for S<'_> {
        fn visit_function(&mut self, f: &Function) {
            let antes = self.estricto;
            self.estricto |= f.body.as_ref().is_some_and(|b| tiene_use_strict(&b.stmts));
            f.visit_children_with(self);
            if !self.estricto && usa_this(f) {
                if let Some(b) = &f.body {
                    let pos = match b.stmts.iter().take_while(|x| es_directiva(x)).last() {
                        Some(d) => d.span().hi,
                        None => b.span.lo + BytePos(1),
                    };
                    self.ins.push(Insercion { pos, cierre: false, texto: THIS_INSERTO.into() });
                }
            }
            self.estricto = antes;
        }
        fn visit_class(&mut self, c: &Class) {
            let antes = self.estricto;
            self.estricto = true;
            c.visit_children_with(self);
            self.estricto = antes;
        }
    }
    insertar(src, SourceKind::Script, |p| {
        let estricto = match p {
            Program::Script(s) => tiene_use_strict(&s.body),
            Program::Module(_) => true,
        };
        let mut ins = Vec::new();
        p.visit_with(&mut S { estricto, ins: &mut ins });
        ramas_texto(p, &mut ins);
        ins
    })
}

/// b7-1b (revision 7.1, H2, opcion (c)): el sufijo de los literales salados.
pub const SAL_LITERAL: &str = "dz";

/// b7-1b (revision 7.1, H2, opcion (c)): el programa con cada literal de cadena EN POSICION DE
/// DATO (elemento de un arreglo, valor de una propiedad, inicializador de una declaracion, lado
/// derecho de una asignacion, objeto de un acceso a miembro como `"abc".substring(...)`) con
/// `SAL_LITERAL` al final; tambien las plantillas sin expresiones (`\`x\``, como las imprime
/// oxc). No se tocan las comparaciones (`typeof x === "u"`), las claves ni (con `Sal::Dato`) los
/// argumentos. 9.2: con `Sal::Amplia`, tambien los de otras posiciones (ver `Sal`).
/// `Some(None)` si no hay ninguno; `None` si no se pudo (`instrumentar`).
pub fn salar_literales(src: &str, kind: SourceKind, sal: Sal) -> Option<Option<String>> {
    use swc_ecma_ast::{
        ArrayLit, ArrowExpr, AssignExpr, AssignPat, BinExpr, BinaryOp, ArrowFunctionBody, CallExpr, Callee, CondExpr, KeyValueProp, MemberExpr, NewExpr,
        ReturnStmt, VarDeclarator,
    };
    fn salar(e: &mut Expr) -> bool {
        match e {
            Expr::Lit(Lit::Str(x)) => {
                let v = format!("{}{SAL_LITERAL}", x.value.to_string_lossy());
                x.value = v.as_str().into();
                x.raw = None;
                true
            }
            Expr::Tpl(t) if t.exprs.is_empty() && t.quasis.len() == 1 => {
                let q = &mut t.quasis[0];
                let (Some(c), raw) = (q.cooked.as_ref(), q.raw.as_str()) else {
                    return false;
                };
                let cooked = format!("{}{SAL_LITERAL}", c.to_string_lossy());
                let raw = format!("{raw}{SAL_LITERAL}");
                q.cooked = Some(cooked.as_str().into());
                q.raw = raw.as_str().into();
                true
            }
            _ => false,
        }
    }
    // `.0`: salo algo; `.1`: `Sal::Amplia`.
    struct S(bool, bool);
    impl VisitMut for S {
        fn visit_mut_call_expr(&mut self, n: &mut CallExpr) {
            n.visit_mut_children_with(self);
            // El especificador de `require`/`import()` no: el modulo dejaria de resolver.
            let require = matches!(&n.callee, Callee::Expr(e) if matches!(&**e, Expr::Ident(i) if &*i.sym == "require"));
            if self.1 && !require && !matches!(n.callee, Callee::Import(_)) {
                for a in n.args.iter_mut().filter(|a| a.spread.is_none()) {
                    self.0 |= salar(&mut a.expr);
                }
            }
        }
        fn visit_mut_new_expr(&mut self, n: &mut NewExpr) {
            n.visit_mut_children_with(self);
            if self.1 {
                for a in n.args.iter_mut().flatten().filter(|a| a.spread.is_none()) {
                    self.0 |= salar(&mut a.expr);
                }
            }
        }
        fn visit_mut_cond_expr(&mut self, n: &mut CondExpr) {
            n.visit_mut_children_with(self);
            if self.1 {
                self.0 |= salar(&mut n.cons);
                self.0 |= salar(&mut n.alt);
            }
        }
        fn visit_mut_bin_expr(&mut self, n: &mut BinExpr) {
            n.visit_mut_children_with(self);
            if self.1 && matches!(n.op, BinaryOp::LogicalOr | BinaryOp::LogicalAnd | BinaryOp::NullishCoalescing) {
                self.0 |= salar(&mut n.right);
            }
        }
        fn visit_mut_return_stmt(&mut self, n: &mut ReturnStmt) {
            n.visit_mut_children_with(self);
            if let (true, Some(e)) = (self.1, &mut n.arg) {
                self.0 |= salar(e);
            }
        }
        fn visit_mut_arrow_expr(&mut self, n: &mut ArrowExpr) {
            n.visit_mut_children_with(self);
            if let (true, ArrowFunctionBody::Expr(e)) = (self.1, &mut *n.body) {
                self.0 |= salar(e);
            }
        }
        fn visit_mut_assign_pat(&mut self, n: &mut AssignPat) {
            n.visit_mut_children_with(self);
            if self.1 {
                self.0 |= salar(&mut n.right);
            }
        }
        fn visit_mut_array_lit(&mut self, n: &mut ArrayLit) {
            n.visit_mut_children_with(self);
            for e in n.elems.iter_mut().flatten().filter(|e| e.spread.is_none()) {
                self.0 |= salar(&mut e.expr);
            }
        }
        fn visit_mut_key_value_prop(&mut self, n: &mut KeyValueProp) {
            n.visit_mut_children_with(self);
            self.0 |= salar(&mut n.value);
        }
        fn visit_mut_var_declarator(&mut self, n: &mut VarDeclarator) {
            n.visit_mut_children_with(self);
            if let Some(e) = &mut n.init {
                self.0 |= salar(e);
            }
        }
        fn visit_mut_assign_expr(&mut self, n: &mut AssignExpr) {
            n.visit_mut_children_with(self);
            self.0 |= salar(&mut n.right);
        }
        fn visit_mut_member_expr(&mut self, n: &mut MemberExpr) {
            n.visit_mut_children_with(self);
            self.0 |= salar(&mut n.obj);
        }
    }
    instrumentar(src, kind, |p| {
        let mut s = S(false, sal == Sal::Amplia);
        p.visit_mut_with(&mut s);
        s.0
    })
}

/// 9.2 (H2 de la revision del bloque 8): que literales sala `salar_literales`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sal {
    /// Los de posicion de dato (b7-1b). Contenido y solo sloppy.
    Dato,
    /// Ademas los argumentos de una llamada o de `new` (no el especificador de `require` ni de
    /// `import()`), las ramas de `?:`, el lado derecho de `||`, `&&` y `??`, el de `return`, el
    /// cuerpo expresion de una flecha y el valor por defecto de un parametro o de un patron.
    /// Profundidad: `String("")` y `c ? "a" : "b"` tambien son la tabla de la coincidencia.
    Amplia,
}

/// 9.2 (M1 de la revision del bloque 8): como dio `Explica` `con_literales_salados`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Salado {
    /// El re-chequeo sobre el original salado tambien dio `Explica`.
    Confirmado,
    /// No habia literales que salar: no se confirmo nada.
    SinLiterales,
}

impl Salado {
    /// Lo que se agrega a la linea de la traza (meta.txt) de un `Explica` salado.
    pub fn traza(self) -> &'static str {
        match self {
            Salado::Confirmado => "confirmado con los literales salados",
            Salado::SinLiterales => "sin literales que salar: no se confirmo con la sal",
        }
    }
}

/// 9.2 (H2 de la revision del bloque 8, profundidad): la sal tiene que LLEGAR a lo que B imprime
/// distinto de A. Cada linea de B (con los nombres de la celda, `oracle::nombres_de_celda`, como
/// `localizada_con_marca`) que difiere de la de A en el mismo lugar (o que A no tiene) tiene que
/// traer, en B salado (`bs`), mas `dz` que en
/// B (sin distinguir mayusculas: `toUpperCase` la conserva), y B salado tiene que imprimir las
/// mismas lineas que B. Si no, la sal toco otra cosa (una etiqueta, otra linea) o nada, y que el
/// salado siga dando `Explica` no prueba que la coincidencia de B con A' venga de un dato: un bug
/// que deja en B un numero, un booleano o un valor calculado que no cambia con la sal la sigue
/// dando (MEDIDO con oxc 0.153.0: `[0, bug.length][i]`, `[true, Number.isInteger(5e-324)][i]`).
/// Sin lineas distintas (A y B difieren solo en el estado o en lo que A imprime de mas), tampoco.
/// Integracion 9.1 + 9.2: con keep_names (9.1) una linea que difiere solo en un nombre que imprime
/// `util.inspect` tambien difiere, y la sal no llega a un nombre: Inconcluso, como un numero (lado
/// seguro; MEDIDO: el positivo b9-int/sonda-sal k2, la funcion impresa elegida por el desborde, pasa
/// de known/ a div/ en carril_a). Sin keep_names los nombres se borran, como antes.
/// `Err(nota)`: no llega.
pub fn sal_alcanza(a: &runner::Exec, b: &runner::Exec, bs: &runner::Exec) -> Result<(), String> {
    let lines = |e: &runner::Exec| oracle::nombres_de_celda(&e.stdout).lines().map(String::from).collect::<Vec<_>>();
    let (la, lb, ls) = (lines(a), lines(b), lines(bs));
    if ls.len() != lb.len() {
        return Err(format!("B salado imprime {} lineas y B {}: la sal cambio lo que B imprime y no se alinea", ls.len(), lb.len()));
    }
    let distintas: Vec<usize> = (0..lb.len()).filter(|&i| la.get(i) != Some(&lb[i])).collect();
    if distintas.is_empty() {
        return Err("B no imprime ninguna linea distinta de A (difieren en el estado o en lo que A imprime de mas): la sal no puede llegar a la coincidencia".into());
    }
    let sales = |l: &str| l.to_lowercase().matches(SAL_LITERAL).count();
    match distintas.iter().find(|&&i| sales(&ls[i]) <= sales(&lb[i])) {
        Some(i) => Err(format!(
            "la sal no llega a la linea {} de B, que difiere de A: la coincidencia de B con el original modificado no sale de un literal salado",
            i + 1
        )),
        None => Ok(()),
    }
}

/// b7-1b (revision 7.1, H2, opcion (c)): para las familias sin sensor de uso completo (contenido:
/// el texto de una funcion del programa; solo sloppy: el `arguments` atado), un `Explica` se
/// confirma con el original SALADO (`salar_literales`): se minifica igual, se corren A y B
/// salados y `recheck` (el mismo re-chequeo) tiene que dar Explica tambien ahi. Si el bug deja
/// justo el valor por defecto, con otro valor ya no coincide (B != A'). Sin literales que salar,
/// no hay nada que mirar: `Ok(Salado::SinLiterales)` (contenido y solo sloppy lo toman como
/// Explica; 9.2: profundidad, como Inconcluso). b7-1c (revision 7.1b, S1): si el salado no
/// diverge, Inconcluso (la sal cambio el camino; antes, Explica). 1 minify + 2 node + el
/// re-chequeo, solo en los candidatos a Explica de esas familias. Heuristica: un valor por
/// defecto que no viene de un literal (`[].join()`) no cambia con la sal.
/// 9.2 (M1): `Ok(Salado)` es el Explica, confirmado o sin literales; `Err(nota)`, el Inconcluso
/// (el NoExplica del salado tambien es Inconcluso). `sal`: que literales se salan.
pub fn con_literales_salados(
    r: &runner::Runner,
    medido: (backend::Minifier, minify::Preset, SourceKind),
    original: &str,
    sal: Sal,
    recheck: impl FnOnce(&str, &str, &runner::Exec, &runner::Exec) -> Recheck,
) -> Result<Salado, String> {
    con_literales_salados_de(r, medido, original, sal, Lados::Minificado, recheck)
}

/// 10.1b: que dos textos del original salado compara `con_literales_salados_de`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lados {
    /// El original que imprime el backend y el minificado (A contra B: `clasificar`).
    Minificado,
    /// `--crudo` (`main::comparar_crudo`): el texto salado hace de crudo y el original que el
    /// backend imprime de el, de A. El salado lo reimprime SWC (`analisis::salar`), asi que puede
    /// no conservar la forma del crudo que dispara el bug del parser o del codegen; si la pierde,
    /// el salado ya no diverge y da `Err` (inconcluso: el crudo va a codegen/, el lado seguro).
    Crudo,
}

/// `con_literales_salados` con los dos textos que se comparan (`lados`).
pub fn con_literales_salados_de(
    r: &runner::Runner,
    (minifier, preset, kind): (backend::Minifier, minify::Preset, SourceKind),
    original: &str,
    sal: Sal,
    lados: Lados,
    recheck: impl FnOnce(&str, &str, &runner::Exec, &runner::Exec) -> Recheck,
) -> Result<Salado, String> {
    let salado = match crate::analisis::salar(original, kind, sal) {
        Ok(Some(Some(s))) => s,
        Ok(Some(None)) => return Ok(Salado::SinLiterales),
        Ok(None) => return Err("no pude salar los literales del original (no parsea con SWC)".into()),
        Err(e) => {
            crate::analisis::registrar("re-chequeo (literales salados)", &e);
            return Err(format!("fallo el analisis de difuzz: {}", e.lines().next().unwrap_or_default()));
        }
    };
    let cfg = preset.build(minifier, kind);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&salado, &cfg) else {
        return Err("no pude minificar el original con los literales salados".into());
    };
    let (o2, m2) = match lados {
        Lados::Minificado => (o2, m2),
        Lados::Crudo => (salado, o2),
    };
    let (a2, b2) = (r.run(&o2), r.run(&m2));
    // b7-1c (revision 7.1b, S1): esto se llama solo con un Explica del original, asi que el
    // original diverge. Si el salado no, la sal cambio el camino (un literal que guarda la rama
    // del bug: `modo === "x" ? bug : ...`) y el salado no confirma nada: antes daba Explica y el
    // bug real iba a known/ (x5-sloppyargs-sal-guarda, x5b-tostring-sal-guarda).
    if matches!(oracle::compare(&a2, &b2), oracle::Verdict::Same) {
        return Err("con los literales salados (otro valor por defecto) el original ya no diverge: la sal cambio el camino y no se puede confirmar".into());
    }
    match recheck(&o2, &m2, &a2, &b2) {
        Recheck::Explica => Ok(Salado::Confirmado),
        Recheck::NoExplica => Err(
            "con los literales salados (otro valor por defecto) la asuncion ya no explica la divergencia: la coincidencia con el valor por defecto no prueba nada".into(),
        ),
        Recheck::Inconcluso(n) => Err(format!("con los literales salados (otro valor por defecto): {n}")),
    }
}

/// R2-H10: el nombre que toma el binding `arguments` en la variante estricta.
pub const ARGUMENTS_RENOMBRADO: &str = "__dz_arguments";

/// R2-H10: el programa (carril A) con cada binding llamado `arguments` renombrado
/// a `ARGUMENTS_RENOMBRADO`, junto con las referencias que lo ven. Ligar
/// `arguments` es error temprano en estricto (`var arguments` no compila con
/// "use strict"), asi que sin esto el re-chequeo sloppy de un caso asi no podia
/// ni empezar (terser collapse_vars_arguments). Alcance: en una funcion no flecha
/// (o el tope del archivo, que en CommonJS es la funcion envoltorio) que liga
/// `arguments` como parametro, `var` o declaracion de funcion, se renombra todo
/// identificador `arguments` de su cuerpo, entrando en flechas (no tienen
/// `arguments` propio) y no en otras funciones no flecha (tienen el suyo, salvo
/// que tambien lo liguen). `None` si no parsea, si no hay nada que renombrar, si
/// el nombre nuevo ya aparece en el fuente o si renombrar cambiaria una clave de
/// objeto (`{ arguments }`, `var { arguments } = o`).
/// Lo que no cubre (p. ej. `let arguments` en un bloque, o en un constructor)
/// queda sin renombrar: la variante sigue sin compilar en estricto y el
/// re-chequeo queda inconcluso, como antes. El renombre es un cambio del
/// programa: lo que pueda cambiar (`.name` inferido de `var arguments = function
/// () {}`, `eval("arguments")`) lo juzga `veredicto` con el chequeo de camino.
pub fn rename_arguments(src: &str) -> Option<String> {
    use swc_ecma_ast::{ArrowExpr, Constructor, FnDecl, Function, ObjectPatProp, Pat, Prop, Script, Stmt, VarDecl, VarDeclKind};
    use swc_ecma_visit::{Visit, VisitWith};

    const NOMBRE: &str = "arguments";
    if src.contains(ARGUMENTS_RENOMBRADO) {
        return None;
    }
    /// Algun binding `arguments` en un patron (sin entrar en expresiones:
    /// defaults ni claves computadas).
    struct Liga(bool);
    impl Visit for Liga {
        fn visit_binding_ident(&mut self, b: &swc_ecma_ast::BindingIdent) {
            self.0 |= &*b.id.sym == NOMBRE;
        }
        fn visit_assign_pat_prop(&mut self, p: &swc_ecma_ast::AssignPatProp) {
            self.0 |= &*p.key.id.sym == NOMBRE;
        }
        fn visit_expr(&mut self, _: &swc_ecma_ast::Expr) {}
    }
    fn liga(p: &Pat) -> bool {
        let mut l = Liga(false);
        p.visit_with(&mut l);
        l.0
    }
    /// El scope de variables de una funcion liga `arguments`: `var` a cualquier
    /// profundidad o declaracion de funcion, sin entrar en otras funciones ni
    /// en flechas (que tienen su propio scope de `var`).
    struct Declara(bool);
    impl Visit for Declara {
        fn visit_function(&mut self, _: &Function) {}
        fn visit_constructor(&mut self, _: &Constructor) {}
        fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
        fn visit_fn_decl(&mut self, f: &FnDecl) {
            self.0 |= &*f.ident.sym == NOMBRE;
        }
        fn visit_var_decl(&mut self, v: &VarDecl) {
            if v.kind == VarDeclKind::Var {
                self.0 |= v.decls.iter().any(|d| liga(&d.name));
            }
            v.visit_children_with(self);
        }
    }
    fn declara(stmts: &[Stmt]) -> bool {
        let mut d = Declara(false);
        stmts.iter().for_each(|s| s.visit_with(&mut d));
        d.0
    }
    struct R {
        /// Por cada funcion no flecha abierta: si renombra en su cuerpo.
        pila: Vec<bool>,
        hecho: bool,
        no_se: bool,
    }
    impl R {
        fn activo(&self) -> bool {
            self.pila.last().copied().unwrap_or(false)
        }
    }
    impl VisitMut for R {
        fn visit_mut_script(&mut self, s: &mut Script) {
            self.pila.push(declara(&s.body));
            s.visit_mut_children_with(self);
            self.pila.pop();
        }
        fn visit_mut_function(&mut self, f: &mut Function) {
            let cuerpo = f.body.as_ref().is_some_and(|b| declara(&b.stmts));
            self.pila.push(cuerpo || f.params.iter().any(|p| liga(&p.pat)));
            f.visit_mut_children_with(self);
            self.pila.pop();
        }
        fn visit_mut_constructor(&mut self, c: &mut Constructor) {
            // Sin renombrar adentro (parametros con TS y propiedades): si liga
            // `arguments`, la variante no compila y queda inconcluso.
            self.pila.push(false);
            c.visit_mut_children_with(self);
            self.pila.pop();
        }
        fn visit_mut_ident(&mut self, i: &mut swc_ecma_ast::Ident) {
            if self.activo() && &*i.sym == NOMBRE {
                i.sym = ARGUMENTS_RENOMBRADO.into();
                self.hecho = true;
            }
        }
        fn visit_mut_prop(&mut self, p: &mut Prop) {
            if matches!(p, Prop::Shorthand(i) if &*i.sym == NOMBRE) && self.activo() {
                self.no_se = true;
            }
            p.visit_mut_children_with(self);
        }
        fn visit_mut_object_pat_prop(&mut self, p: &mut ObjectPatProp) {
            if matches!(p, ObjectPatProp::Assign(a) if &*a.key.id.sym == NOMBRE) && self.activo() {
                self.no_se = true;
            }
            p.visit_mut_children_with(self);
        }
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
            let mut program = backend::swc::parse_program(&fm, SourceKind::Script)?;
            let mut r = R { pila: Vec::new(), hecho: false, no_se: false };
            program.visit_mut_with(&mut r);
            if !r.hecho || r.no_se {
                return None;
            }
            emit(cm, &program.apply(fixer(None)))
        })
    }))
    .ok()
    .flatten()
}

fn emit(cm: Lrc<SourceMap>, program: &Program) -> Option<String> {
    let mut buf = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: swc_ecma_codegen::Config::default(),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm, "\n", &mut buf, None),
        };
        emitter.emit_program(program).ok()?;
    }
    String::from_utf8(buf).ok()
}

#[cfg(test)]
mod tests {
    use super::{globals_matter, neutralize_violaciones, neutralize_with, offset_utf16, violaciones_matter, with_matters, Camino, Recheck};
    use crate::backend::Minifier;
    use crate::minify::{Preset, SourceKind};
    use crate::runner::{Exec, Preload, Runner, Status, THROW_EXIT};

    fn ex(status: Status, stdout: &str) -> Exec {
        let ctl = if status == Status::Exited(THROW_EXIT) { "throw X\n" } else { "" };
        Exec {
            status,
            stdout: stdout.into(),
            stderr: String::new(),
            flags: Vec::new(),
            elapsed: std::time::Duration::ZERO,
            ctl: ctl.into(),
        }
    }

    /// b7-1: `veredicto_neutralizado` con un B que no pasa por la violacion (lo de antes).
    fn veredicto_igual(a: &Exec, b: &Exec, a2: &Exec, b2: &Exec, salada: Option<&Exec>) -> Recheck {
        super::veredicto_neutralizado(a, b, a2, b2, salada, || Camino::Igual)
    }

    /// b7-1 (revision n2, IMPORTANTE 1): B == A' y B' == A', pero B pasa por la violacion
    /// (r01: `let s = ""` y el bug deja `""` en el catch de la TDZ): ya no es Explica.
    /// `camino` solo se consulta si todo lo demas da Explica.
    #[test]
    fn veredicto_exige_que_b_no_pase_por_la_violacion() {
        use super::veredicto_neutralizado as v;
        let ok = |s: &str| ex(Status::Exited(0), s);
        let (a, b) = (ok("\"bc\"\n"), ok("\"\"\n"));
        let r = v(&a, &b, &b, &b, None, || Camino::Distinto("el sensor ve en B la violacion tdz 1:16".into()));
        assert!(matches!(r, Recheck::Inconcluso(ref n) if n.contains("todavia pasa por la violacion") && n.contains("tdz 1:16")), "{r:?}");
        let r = v(&a, &b, &b, &b, None, || Camino::NoSe("sin cobertura".into()));
        assert!(matches!(r, Recheck::Inconcluso(ref n) if n.contains("no pude ver") && n.contains("sin cobertura")), "{r:?}");
        assert_eq!(v(&a, &b, &b, &b, None, || Camino::Igual), Recheck::Explica);
        let nunca = || -> Camino { panic!("no hacia falta mirar el camino de B") };
        assert_eq!(v(&ok("1\n"), &ok("2\n"), &ok("1\n"), &ok("2\n"), None, nunca), Recheck::NoExplica);
        assert!(matches!(v(&ok("1\n"), &ok("2\n"), &ok("1\n"), &ok("1\n"), None, nunca), Recheck::Inconcluso(ref n) if n.contains("B != A'")));
    }

    /// b7-1: el sensor en B (lineas `viol`), por clase; sin el sensor entero, no se sabe.
    #[test]
    fn camino_por_el_sensor() {
        use super::camino_sensor;
        let mut b = ex(Status::Exited(0), "");
        assert_eq!(camino_sensor(&b, true, None), Camino::Igual);
        b.ctl = "viol tdz 1:16\nflags tdz_atrapada\n".into();
        assert!(matches!(camino_sensor(&b, true, None), Camino::Distinto(ref n) if n.contains("tdz 1:16")));
        assert_eq!(camino_sensor(&b, true, Some(&["extends"])), Camino::Igual);
        assert!(matches!(camino_sensor(&b, false, None), Camino::NoSe(_)));
        b.ctl = "viol extends 0:10\nsensor_tope\n".into();
        assert!(matches!(camino_sensor(&b, true, Some(&["extends"])), Camino::NoSe(ref n) if n.contains("sensor_tope")));
        // Integracion 7.1 + 7.4 (rev-int H1): heredar las violaciones de la corrida con el
        // sensor (7.4) no le da a B sin el sensor un camino: sigue sin saberse.
        let mut sin = ex(Status::Exited(0), "");
        sin.heredar_violaciones(&ex(Status::Exited(0), ""));
        assert!(matches!(camino_sensor(&sin, false, None), Camino::NoSe(_)));
    }

    /// b7-1: la comparacion de coberturas exige que las corridas con cobertura sean las de B y B'.
    #[test]
    fn comparar_caminos_por_cobertura() {
        use super::comparar_caminos;
        let cov = |s: &str, c: &str| {
            let mut e = ex(Status::Exited(0), s);
            e.ctl = format!("flags \ncobertura {c}\n");
            e
        };
        let (b, b2) = (ex(Status::Exited(0), "\"\"\n"), ex(Status::Exited(0), "\"\"\n"));
        assert_eq!(comparar_caminos(&b, &cov("\"\"\n", "ab 3"), Some(&b2), &cov("\"\"\n", "ab 3")), Camino::Igual);
        assert!(matches!(comparar_caminos(&b, &cov("\"\"\n", "ab 3"), Some(&b2), &cov("\"\"\n", "cd 4")), Camino::Distinto(_)));
        assert!(matches!(comparar_caminos(&b, &cov("x\n", "ab 3"), Some(&b2), &cov("\"\"\n", "ab 3")), Camino::NoSe(_)));
        assert!(matches!(comparar_caminos(&b, &cov("\"\"\n", "ab 3"), Some(&b2), &cov("y\n", "ab 3")), Camino::NoSe(_)));
        assert!(matches!(comparar_caminos(&b, &ex(Status::Exited(0), "\"\"\n"), None, &cov("y\n", "ab 3")), Camino::NoSe(_)));
        // b7-1b (revision 7.1, H2): el sensor de uso manda sobre el camino (mismo camino, pero B
        // toco la asuncion); lo que no se pudo sensar, no se sabe.
        let mut uso = cov("\"\"\n", "ab 3");
        uso.ctl.push_str("uso global window\n");
        let r = comparar_caminos(&b, &cov("\"\"\n", "ab 3"), Some(&b2), &uso);
        assert!(matches!(r, Camino::Distinto(ref n) if n.contains("B usa la asuncion: global window")), "{r:?}");
        let mut no = cov("\"\"\n", "ab 3");
        no.ctl.push_str("uso? builtins p2 includes\n");
        let r = comparar_caminos(&b, &no, Some(&b2), &cov("\"\"\n", "ab 3"));
        assert!(matches!(r, Camino::NoSe(ref n) if n.contains("builtins p2 includes")), "{r:?}");
    }

    /// b1-b: solo es concluyente si A' termina y B' == A'; n-principio: y B == A' (los
    /// escenarios del viejo `veredicto`, con lo que dan ahora).
    #[test]
    fn veredicto_concluyente() {
        let v = veredicto_igual;
        let ok = |s: &str| ex(Status::Exited(0), s);
        let th = |s: &str| ex(Status::Exited(THROW_EXIT), s);
        let inconcluso = |r: Recheck| matches!(r, Recheck::Inconcluso(ref n) if n.contains("B != A'"));
        // B ya se comporta como A' (03-global-positivo): Explica, como antes.
        assert_eq!(v(&ok("no hay\n"), &ok("hay\n"), &ok("hay\n"), &ok("hay\n"), None), Recheck::Explica);
        // n-principio: lo que antes era Explica y ahora es inconcluso (div/).
        // - Mismo camino (A' == A): n24/n30/n34 (A' == A por construccion, el bug en la rama
        //   que solo corre con la violacion).
        assert!(inconcluso(v(&ok("1\n"), &ok("2\n"), &ok("1\n"), &ok("1\n"), None)));
        // - Mismo punto, otro tipo de throw (sloppy: TypeError, estricto: ReferenceError;
        //   dead_code_2_should_warn) con B que no es A'.
        let (a, a2) = (th("!dz throw TypeError\n"), th("!dz throw ReferenceError\n"));
        assert!(inconcluso(v(&a, &th("!dz throw Error\n"), &a2, &th("!dz throw ReferenceError\n"), None)));
        // - `with` (issue_1105): B no es ni A ni A' y solo difiere en las lineas que el scope
        //   cambia (`localizada`). Con la misma forma pasaba n23 (el bug en la rama del scope).
        assert!(inconcluso(v(&ok("u\n3\n"), &ok("u\n4\n"), &ok("u\nu\n"), &ok("u\nu\n"), None)));
        assert!(inconcluso(v(&ok("-2\n"), &ok("NaN\n"), &ok("nok\n"), &ok("nok\n"), None)));
        // I1 (revision-b456, r08-readonly-getter): A' lanza en la linea donde B difiere.
        assert!(inconcluso(v(&ok("1 PASS\n"), &ok("1 FAIL\n"), &th("!dz throw TypeError\n"), &th("!dz throw TypeError\n"), None)));
        // La divergencia sigue con la asuncion neutralizada.
        assert_eq!(v(&ok("1\n"), &ok("2\n"), &ok("1\n"), &ok("2\n"), None), Recheck::NoExplica);
        // 10-rt-*-miente: A' toma otra rama y B no es A': un bug real no se esconde.
        assert!(inconcluso(v(&ok("rest\nfin\n"), &ok("fin\n"), &ok("navegador\nfin\n"), &ok("navegador\nfin\n"), None)));
        // R2-H02: un Timeout en la variante no es evidencia.
        let r = v(&ok("1\n"), &ok("2\n"), &ex(Status::Timeout, ""), &ok("1\n"), None);
        assert!(matches!(r, Recheck::Inconcluso(ref n) if n.contains("Timeout")), "{r:?}");
        let r = v(&ok("1\n"), &ok("2\n"), &ok("1\n"), &ex(Status::Timeout, ""), None);
        assert!(matches!(r, Recheck::Inconcluso(_)), "{r:?}");
    }

    /// Paso 7 (revision-tint B1): en los re-chequeos por neutralizacion solo B == A' es
    /// concluyente; `mismo_camino` y `localizada` ya no (el bug puede estar en lo neutralizado).
    #[test]
    fn veredicto_neutralizado_exige_b_igual_a_a2() {
        use super::{localizada_con_marca, FIJO_MARCO, FIJO_TOSTRING};
        let veredicto_neutralizado = veredicto_igual;
        let ok = |s: &str| ex(Status::Exited(0), s);
        // B ya se comporta como A' (16-builtins-positivo, keep_comparisons): Explica.
        assert_eq!(veredicto_neutralizado(&ok("pisado\n"), &ok("ABC\n"), &ok("ABC\n"), &ok("ABC\n"), None), Recheck::Explica);
        // 20-p01: A' == A (el pisado devuelve lo mismo que el original) y B difiere: antes
        // `mismo_camino`, Explica; ahora inconcluso.
        let r = veredicto_neutralizado(&ok("true\n"), &ok("false\n"), &ok("true\n"), &ok("true\n"), None);
        assert!(matches!(r, Recheck::Inconcluso(ref n) if n.contains("B != A'")), "{r:?}");
        // r05/r09: la linea del bug cambia en A' (no corre): antes `localizada`, ahora inconcluso.
        let r = veredicto_neutralizado(&ok("-2\n"), &ok("NaN\n"), &ok("none\n"), &ok("none\n"), None);
        assert!(matches!(r, Recheck::Inconcluso(_)), "{r:?}");
        // La divergencia sigue: NoExplica.
        assert_eq!(veredicto_neutralizado(&ok("1\n"), &ok("2\n"), &ok("3\n"), &ok("4\n"), None), Recheck::NoExplica);
        // Contenido con marca: el toString impreso (17-tostring-positivo) y los marcos del stack.
        let fuente = "function suma(a, b) { return a + b; }";
        let (a, b) = (ok(&format!("f: {fuente}\nfin\n")), ok("f: function suma(n,r){return n+r}\nfin\n"));
        let a2 = ok(&format!("f: {FIJO_TOSTRING}\nfin\n"));
        let salada = ok(&format!("f: {fuente}/*dz*/\nfin\n"));
        assert!(localizada_con_marca(&a, &b, &a2, &salada));
        assert_eq!(veredicto_neutralizado(&a, &b, &a2, &a2, Some(&salada)), Recheck::Explica);
        // ... sin la sal en esa linea de A (el contenido decide un camino, no se imprime): no.
        assert!(!localizada_con_marca(&a, &b, &a2, &a));
        // ... el resto de la linea distinto (bug al lado del contenido): no.
        assert!(!localizada_con_marca(&a, &ok("g: function suma(n,r){return n+r}\nfin\n"), &a2, &salada));
        // r07/p03: la linea del bug no muestra contenido en A' (`none`): no.
        assert!(!localizada_con_marca(&ok("-2\n"), &ok("NaN\n"), &ok("none\n"), &ok("-2\n")));
        let (a, b) = (ok("TypeError: x\n    at falla (/tmp/p.js:3:15)\n"), ok("TypeError: x\n    at falla (/tmp/p.js:1:30)\n"));
        let a2 = ok(&format!("TypeError: x\n{FIJO_MARCO}\n"));
        assert!(localizada_con_marca(&a, &b, &a2, &a));
        assert!(localizada_con_marca(&ok("    at /tmp/p.js:3:15\n"), &ok("    at /tmp/p.js:1:9\n"), &ok(&format!("{FIJO_MARCO}\n")), &a));
        // n-principio (n-ruido r03, r02/r09): otro nombre (otra funcion, o los marcos corridos
        // porque cambio la cuenta o el orden) no es solo contenido. Antes Explica; tambien el
        // mangling de core_defaults (`at falla` -> `at r`), que pasa a div/.
        assert!(!localizada_con_marca(&a, &ok("TypeError: x\n    at r (/tmp/p.js:1:30)\n"), &a2, &a));
        let a2 = ok(&format!("Error: x\n{FIJO_MARCO}\n{FIJO_MARCO}\n"));
        let a = ok("Error: x\n    at r (/tmp/p.js:2:30)\n    at Object.<anonymous> (/tmp/p.js:3:13)\n");
        let b = ok("Error: x\n    at r (/tmp/p.js:1:9)\n    at r (/tmp/p.js:1:20)\n");
        assert!(!localizada_con_marca(&a, &b, &a2, &a));
        assert_eq!(super::cabeza_marco("    at new Foo (/tmp/p.js:1:2)"), Some("new Foo"));
        assert_eq!(super::cabeza_marco("    at /tmp/p.js:1:2"), Some(""));
        // 20-p04: una linea que empieza con `    at ` pero no es un marco de V8.
        assert!(!localizada_con_marca(&ok("    at -2\n"), &ok("    at NaN\n"), &ok(&format!("{FIJO_MARCO}\n")), &ok("    at -2\n")));
        assert!(super::es_marco("    at Array.map (<anonymous>)") && super::es_marco("    at /tmp/p.js:1:2"));
        assert!(!super::es_marco("    at f (x.js:1:-2)") && !super::es_marco("    at "));
        assert!(super::encaja(&["a ", " b"], "a xyz b") && !super::encaja(&["a ", " b"], "a b") && super::encaja(&["", ""], ""));
    }

    #[test]
    fn neutraliza_with() {
        let out = neutralize_with("var o = {a: 1};\nwith (o) { console.log(a); }\n", SourceKind::Script).unwrap();
        assert!(out.contains("{\n    o;\n    {\n        console.log(a);\n    }\n}"), "{out}");
        assert!(neutralize_with("console.log(1);\n", SourceKind::Script).is_none());
    }

    /// b7-1: los interruptores en el minificado: el objeto de cada `with` y la clave de cada
    /// definicion de helper de coercion (las mismas que `neutralize_coercion`); las lecturas y
    /// los patrones no. Tres respuestas: instrumentado, nada que cambiar, no se pudo.
    #[test]
    fn interruptores_en_el_minificado() {
        use super::{interruptores_coercion, interruptores_with};
        let k = SourceKind::Script;
        let out = interruptores_with("var o={a:1};with(o)console.log(a);with({})x;", k).unwrap().unwrap();
        assert!(out.contains("with (__difuzzNeutroWith(o))") && out.contains("with (__difuzzNeutroWith({}))"), "{out}");
        assert_eq!(interruptores_with("console.log(1);", k), Some(None));
        assert_eq!(interruptores_with("let = ;", k), None);
        assert_eq!(interruptores_with("with(o)__difuzzNeutroWith;", k), None);
        // b7-1b: con los contadores de rama; sin `with`, nada que cambiar aunque haya `&&=`.
        assert_eq!(interruptores_with("with(o)t&&=k;", k).unwrap().unwrap(), "with (__difuzzNeutroWith(o))t &&= (__difuzzNeutroRama(0), k);\n");
        assert_eq!(interruptores_with("t&&=k;", k), Some(None));
        // b7-1b (revision 7.1, H2 d03): y el sensor de uso (al principio de cada helper que es una
        // funcion; envolviendo el valor de los demas) y los contadores de rama.
        let src = "var o = { valueOf() { return 1; }, toString: f, [Symbol.toPrimitive]: g, x: 1, get valueOf() { return 2 }, toString: function () { 'use strict'; return 3 }, valueOf: () => 4 };\nclass C { toString() {} }\nC.prototype.valueOf = h;\no[Symbol.toPrimitive] = k;\nvar { toString: t } = o;\nconsole.log(o.valueOf());\nt &&= 1;\n";
        let out = interruptores_coercion(src, k).unwrap().unwrap();
        for n in [
            "[__difuzzNeutroClave(\"valueOf\")] () {\n        __difuzzNeutroUso(\"valueOf\");\n        return 1;",
            "[__difuzzNeutroClave(\"toString\")]: __difuzzNeutroValor(f, \"toString\")",
            "[__difuzzNeutroClave(Symbol.toPrimitive)]: __difuzzNeutroValor(g, \"toPrimitive\")",
            "get [__difuzzNeutroClave(\"valueOf\")] () {\n        __difuzzNeutroUso(\"valueOf\");",
            "function() {\n        'use strict';\n        __difuzzNeutroUso(\"toString\");",
            "[__difuzzNeutroClave(\"valueOf\")]: ()=>(__difuzzNeutroUso(\"valueOf\"), 4)",
            "class C {\n    [__difuzzNeutroClave(\"toString\")]() {\n        __difuzzNeutroUso(\"toString\");",
            "C.prototype[__difuzzNeutroClave(\"valueOf\")] = __difuzzNeutroValor(h, \"valueOf\")",
            "o[__difuzzNeutroClave(Symbol.toPrimitive)] = __difuzzNeutroValor(k, \"toPrimitive\")",
            "t &&= (__difuzzNeutroRama(0), 1)",
        ] {
            assert!(out.contains(n), "{n}: {out}");
        }
        assert!(out.contains("toString: t") && out.contains("o.valueOf()"), "{out}");
        assert_eq!(interruptores_coercion("var { toString } = o;\n", k), Some(None));
    }

    /// b7-1b (revision 7.1, H1): los contadores de rama donde V8 no separa bloques: el lado
    /// derecho de `&&=`/`||=`/`??=` (identificador o miembro) y los valores por defecto (parametro,
    /// declaracion, asignacion, `for`, `catch`). Una definicion anonima no (NamedEvaluation).
    /// b7-1c (revision 7.1b, P1): insertados en el texto de B (no re-emitido); sacando lo insertado
    /// (lo que hace el preload en `toString`) vuelve exactamente B. Anidados: el de adentro se cierra
    /// antes.
    #[test]
    fn contadores_de_rama() {
        use super::instrumentar_ramas;
        let k = SourceKind::Script;
        let f = super::RAMA_FIN;
        let src = "let e=``,t=1;t&&=e=``;t||=1;t??=f();o.p&&=g();function g(t=e=``){}const[a=1]=[],{b=2}=o;({c=3}=o);[d=4]=o;y||=function(){};z=(q=function(){})=>q;for(const[w=5]of o);try{}catch({m=6}){}a||=b||=c;";
        let out = instrumentar_ramas(src, k).unwrap().unwrap();
        for n in [
            format!("t&&=(__difuzzNeutroRama(0),e=``{f};"),
            format!("t||=(__difuzzNeutroRama(1),1{f};"),
            format!("t??=(__difuzzNeutroRama(2),f(){f};"),
            format!("o.p&&=(__difuzzNeutroRama(3),g(){f};"),
            format!("function g(t=(__difuzzNeutroRama(4),e=``{f}){{}}"),
            format!("const[a=(__difuzzNeutroRama(5),1{f}]=[],{{b=(__difuzzNeutroRama(6),2{f}}}=o"),
            format!("({{c=(__difuzzNeutroRama(7),3{f}}}=o)"),
            format!("[d=(__difuzzNeutroRama(8),4{f}]=o"),
            "y||=function(){}".into(),
            "z=(q=function(){})=>q".into(),
            format!("for(const[w=(__difuzzNeutroRama(9),5{f}]of o)"),
            format!("catch({{m=(__difuzzNeutroRama(10),6{f}}})"),
            format!("a||=(__difuzzNeutroRama(12),b||=(__difuzzNeutroRama(11),c{f}{f};"),
        ] {
            assert!(out.contains(&n), "{n}: {out}");
        }
        assert_eq!(sin_ganchos(&out), src);
        assert_eq!(instrumentar_ramas("if (t) e = 1; t && (e = 1); x = a ?? b;", k), Some(None));
        assert_eq!(instrumentar_ramas("let = ;", k), None);
        assert_eq!(instrumentar_ramas("t &&= __difuzzNeutroRama;", k), None);
        // Con comentarios, saltos de linea y texto no ASCII en el medio: B tal cual mas lo insertado.
        let src = "function s(p = /* ñ */ 'ü' // x\n) { return p; }\nq ??= 1\n";
        let out = instrumentar_ramas(src, k).unwrap().unwrap();
        assert_eq!(sin_ganchos(&out), src, "{out}");
    }

    /// b7-1c: saca lo que insertan `instrumentar_ramas` e `interruptores_sloppy`, como `quitarGanchos`
    /// del preload (preload.js).
    fn sin_ganchos(s: &str) -> String {
        let mut out = s.replace(super::RAMA_FIN, "").replace(super::THIS_INSERTO, "");
        while let Some(i) = out.find("(__difuzzNeutroRama(") {
            let resto = &out[i + "(__difuzzNeutroRama(".len()..];
            let n = resto.find("),").unwrap();
            assert!(resto[..n].bytes().all(|c| c.is_ascii_digit()), "{out}");
            out.replace_range(i..i + "(__difuzzNeutroRama(".len() + n + 2, "");
        }
        out
    }

    /// b7-1b (revision 7.1, H2, opcion (c)): se salan los literales de cadena en posicion de dato
    /// (y las plantillas sin expresiones), no las comparaciones, los argumentos ni las claves.
    #[test]
    fn literales_salados() {
        use super::salar_literales;
        let k = SourceKind::Script;
        let src = "const t = [\"\", \"abc\".substring(1, undefined)], u = `x`;\nx = { k: 'v', \"c\": 1 };\nif (typeof window === \"undefined\") f(\"arg\");\n";
        let out = salar_literales(src, k, super::Sal::Dato).unwrap().unwrap();
        for n in ["\"dz\"", "\"abcdz\".substring(1, undefined)", "u = `xdz`", "k: \"vdz\"", "\"c\": 1", "typeof window === \"undefined\"", "f(\"arg\")"] {
            assert!(out.contains(n), "{n}: {out}");
        }
        assert_eq!(salar_literales("if (a === \"x\") f(\"y\");", k, super::Sal::Dato), Some(None));
        assert_eq!(salar_literales("let = ;", k, super::Sal::Dato), None);
    }

    /// 9.2 (H2 de la revision del bloque 8): la sal amplia de profundidad toca ademas argumentos
    /// (no el de `require` ni el de `import()`), ramas de `?:`, el lado derecho de `||`/`&&`/`??`,
    /// `return`, el cuerpo de una flecha y los valores por defecto; sigue sin tocar comparaciones,
    /// claves ni accesos calculados.
    #[test]
    fn literales_salados_amplia() {
        use super::{salar_literales, Sal};
        let k = SourceKind::Script;
        let src = "const t = [String(\"\"), String(\"abc\").substring(1, undefined)], e = c ? \"a\" : `b`;\nconst r = require(\"fs\"), m = import(\"x\");\nfunction f(p = \"d\") { return \"r\"; }\nconst g = () => \"flecha\";\nx = y || \"o\";\nif (typeof w === \"undefined\") h(\"arg\", ...q);\nnew C(\"n\");\nz = o[\"k\"];\n";
        let out = salar_literales(src, k, Sal::Amplia).unwrap().unwrap();
        for n in [
            "String(\"dz\")",
            "String(\"abcdz\").substring(1, undefined)",
            "\"adz\" : `bdz`",
            "require(\"fs\")",
            "import(\"x\")",
            "\"ddz\"",
            "return \"rdz\"",
            "\"flechadz\"",
            "y || \"odz\"",
            "typeof w === \"undefined\"",
            "h(\"argdz\", ...q)",
            "new C(\"ndz\")",
            "o[\"k\"]",
        ] {
            assert!(out.contains(n), "{n}: {out}");
        }
        // Con la de dato, nada de eso (los argumentos y las ramas quedan como estan).
        assert_eq!(salar_literales("console.log(c ? String(\"\") : \"x\");", k, Sal::Dato), Some(None));
        assert!(salar_literales("console.log(c ? String(\"\") : \"x\");", k, Sal::Amplia).unwrap().is_some());
        assert_eq!(salar_literales("if (a === \"x\" && o[\"k\"]) f(1, true, null, require(\"y\"));", k, Sal::Amplia), Some(None));
    }

    /// 9.2 (H2 de la revision del bloque 8): la sal tiene que llegar a cada linea en que B difiere
    /// de A (mas `dz` en B salado que en B, sin distinguir mayusculas), con las mismas lineas.
    #[test]
    fn la_sal_alcanza() {
        use super::sal_alcanza;
        let ok = |s: &str| ex(Status::Exited(0), s);
        // p01: la linea distinta trae la sal (tambien en mayusculas).
        assert_eq!(sal_alcanza(&ok("desbordo\n"), &ok("cupo\n"), &ok("cupodz\n")), Ok(()));
        assert_eq!(sal_alcanza(&ok("r:\nDESBORDO\n"), &ok("r:\nCUPO\n"), &ok("r:dz\nCUPODZ\n")), Ok(()));
        // n02/n06: la sal toca otra cosa de la misma linea que no es `dz` (una cuenta), o un numero.
        let n = sal_alcanza(&ok("1 \"bc\"\n"), &ok("1 \"\"\n"), &ok("3 \"\"\n"));
        assert!(n.as_ref().is_err_and(|n| n.contains("linea 1")), "{n:?}");
        assert!(sal_alcanza(&ok("2\n"), &ok("0\n"), &ok("0\n")).is_err());
        // n05: la sal llega a otra linea (la etiqueta), no a la que difiere.
        let n = sal_alcanza(&ok("x\n\"bc\"\n"), &ok("x\n\"\"\n"), &ok("xdz\n\"\"\n"));
        assert!(n.as_ref().is_err_and(|n| n.contains("linea 2")), "{n:?}");
        // Una `dz` que ya estaba no cuenta; otra cantidad de lineas, o ninguna linea distinta, no.
        assert!(sal_alcanza(&ok("adz 1\n"), &ok("adz 2\n"), &ok("adz 2\n")).is_err());
        assert!(sal_alcanza(&ok("a\n"), &ok("b\n"), &ok("bdz\nc\n")).is_err_and(|n| n.contains("lineas")));
        assert!(sal_alcanza(&ex(Status::Exited(1), "a\n"), &ok("a\n"), &ok("adz\n")).is_err_and(|n| n.contains("ninguna linea")));
        // Una linea que A no tiene tambien tiene que traer la sal.
        assert_eq!(sal_alcanza(&ok("a\n"), &ok("a\ncupo\n"), &ok("adz\ncupodz\n")), Ok(()));
        assert!(sal_alcanza(&ok("a\n"), &ok("a\n7\n"), &ok("adz\n7\n")).is_err());
    }

    /// b7-1b (revision 7.1, H2 d07): el sensor del `this` de sloppy, en cada funcion sloppy que
    /// usa su propio `this` (tambien desde una flecha), despues de las directivas; no en una
    /// funcion con "use strict" (ni adentro), en una clase ni en una funcion sin `this`.
    #[test]
    fn interruptores_de_sloppy() {
        use super::interruptores_sloppy;
        let src = "function f(){'use x';return this}function g(){'use strict';return function(){return this}}var h=function(){return()=>this},i={m(){return this}};class C{m(){return this}}function j(){return function(){}}t??=1;function k(){'use y'\nreturn this}";
        let out = interruptores_sloppy(src).unwrap().unwrap();
        for n in [
            "function f(){'use x';;__difuzzNeutroThis(this);return this}".to_string(),
            "var h=function(){;__difuzzNeutroThis(this);return()=>this}".into(),
            "m(){;__difuzzNeutroThis(this);return this}}".into(),
            format!("t??=(__difuzzNeutroRama(0),1{};", super::RAMA_FIN),
            "function k(){'use y';__difuzzNeutroThis(this);\nreturn this}".into(),
        ] {
            assert!(out.contains(&n), "{n}: {out}");
        }
        assert_eq!(out.matches("__difuzzNeutroThis").count(), 4, "{out}");
        assert_eq!(sin_ganchos(&out), src);
        assert_eq!(interruptores_sloppy("'use strict';function f(){return this}"), Some(None));
        assert_eq!(interruptores_sloppy("function f(){return 1}"), Some(None));
    }

    /// t-principio: se renombran las definiciones de helpers de coercion (literal,
    /// clase, asignacion; `[Symbol.toPrimitive]` incluido); las lecturas y los
    /// patrones de desestructuracion no.
    #[test]
    fn neutraliza_coercion() {
        use super::neutralize_coercion;
        let src = "var o = { valueOf() { return 1; }, toString: f, [Symbol.toPrimitive]: g, x: 1 };\nclass C { toString() {} }\nC.prototype.valueOf = h;\no[Symbol.toPrimitive] = k;\nvar { toString: t } = o;\nconsole.log(o.valueOf());\n";
        let out = neutralize_coercion(src, SourceKind::Script).unwrap();
        for n in ["__dz_valueOf ()","__dz_toString: f", "__dz_toPrimitive: g", "class C {\n    __dz_toString()", "C.prototype.__dz_valueOf = h", "o.__dz_toPrimitive = k"] {
            assert!(out.contains(n), "{n}: {out}");
        }
        // Lecturas y patrones quedan.
        assert!(out.contains("toString: t") && out.contains("o.valueOf()"), "{out}");
        assert!(neutralize_coercion("var { toString } = o;\nconsole.log(1);\n", SourceKind::Script).is_none());
        assert!(neutralize_coercion("var __dz_x = { valueOf() {} };\n", SourceKind::Script).is_none());
    }

    /// t-principio: las lineas `builtin` del canal, sin repetir.
    #[test]
    fn lee_builtins_pisados() {
        let mut e = ex(Status::Exited(0), "");
        e.ctl = "dz\nbuiltin p1 join\nbuiltin p1 join\nbuiltin o14 max\nflags builtins\n".into();
        assert_eq!(super::builtins_pisados(&e), ["p1 join", "o14 max"]);
    }

    /// R2-H10: el binding `arguments` y lo que lo ve se renombran; el
    /// `arguments` propio de otra funcion no flecha, no.
    #[test]
    fn renombra_arguments() {
        use super::rename_arguments;
        // collapse_vars_arguments: el `arguments` de `inner` es el suyo.
        let out = rename_arguments("var outer=function(){var k=7,arguments=5,inner=function(){console.log(arguments)};inner(k,1)};outer();").unwrap();
        assert!(out.contains("__dz_arguments = 5") && out.contains("console.log(arguments)"), "{out}");
        // Las flechas no tienen `arguments` propio: ven el ligado.
        let out = rename_arguments("function f(){var arguments=1;return()=>arguments}").unwrap();
        assert!(out.contains("__dz_arguments") && !out.replace("__dz_arguments", "").contains("arguments"), "{out}");
        // Parametro: se renombra en su funcion, no en la anidada.
        let out = rename_arguments("function f(arguments){return function(){return arguments}}").unwrap();
        assert!(out.contains("f(__dz_arguments)") && out.contains("return arguments"), "{out}");
        // Nada que renombrar, o renombrar cambiaria una clave: None.
        assert!(rename_arguments("console.log(arguments);").is_none());
        assert!(rename_arguments("function f(){var arguments=1;return{arguments}}").is_none());
        assert!(rename_arguments("function f(){var arguments=1;var{arguments}=o}").is_none());
        assert!(rename_arguments("var __dz_arguments;function f(){var arguments=1}").is_none());
    }

    /// Con node: el re-chequeo saca el flag solo si la divergencia sigue.
    #[test]
    fn rechequeos_en_node() {
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("rechk").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        // Divergencia por leer `window` (oxc borra la lectura): con window
        // definido desaparece: el flag se conserva.
        let o = "try { window; console.log(1); } catch (e) { console.log(2); }\n";
        let m = "try { console.log(1); } catch (e) { console.log(2); }\n";
        assert_eq!(globals_matter(&r, SourceKind::Script, o, m, &r.run(o), &r.run(m)), Recheck::Explica);
        // Una divergencia que no tiene que ver con `document`: el flag se va.
        let o = "typeof document; console.log(1);\n";
        let m = "console.log(2);\n";
        assert_eq!(globals_matter(&r, SourceKind::Script, o, m, &r.run(o), &r.run(m)), Recheck::NoExplica);
        // b1-b (R1-H02, 10-rt-global-miente): el bug esta en la rama "sin window";
        // con el stub el original toma la otra y la divergencia "desaparece". Antes
        // iba a known/; ahora es inconcluso (div/).
        let o = "if (typeof window === \"undefined\") { console.log(\"rama\"); } else { console.log(\"navegador\"); }\nconsole.log(\"fin\");\n";
        let m = "if (typeof window === \"undefined\") {} else { console.log(\"navegador\"); }\nconsole.log(\"fin\");\n";
        let g = globals_matter(&r, SourceKind::Script, o, m, &r.run(o), &r.run(m));
        assert!(matches!(g, Recheck::Inconcluso(ref n) if n.contains("B != A'")), "{g:?}");
        // n-principio (n29-global-rama): el bug en la rama sin `window` (A=-2, B=NaN, A'=nav).
        // Con `veredicto` iba a known/ (`localizada`); ahora B != A': inconcluso.
        let o = "var r = typeof window !== \"undefined\" ? \"nav\" : Math.cbrt(-8);\nconsole.log(r);\n";
        let m = "console.log(typeof window<`u`?`nav`:NaN);\n";
        let (ao, bm) = (r.run(o), r.run(m));
        assert_eq!((ao.stdout.as_str(), bm.stdout.as_str()), ("-2\n", "NaN\n"));
        let g = globals_matter(&r, SourceKind::Script, o, m, &ao, &bm);
        assert!(matches!(g, Recheck::Inconcluso(ref n) if n.contains("B != A'")), "{g:?}");
        // Los globals definidos son stubs: se pueden leer y llamar.
        let e = r.run_with_globals("console.log(typeof window, String(window.location.href), typeof document);\n", "window");
        assert_eq!(e.stdout, "function [stub window.location.href] undefined\n");
        // `with` que no explica nada: sin el scope, el programa diverge igual
        // (no aplica: el minificado se re-hace con oxc, que no diverge aca).
        let w = "var o = { x: 1 };\nwith (o) { console.log(typeof o); }\n";
        let aw = r.run(w);
        assert_eq!(with_matters(&r, (Minifier::Oxc, Preset::CoreDefaults, SourceKind::Script), w, w, &aw, &aw), Recheck::Explica);
        // O4 (oxc #24331: `delete (0/0)` -> `delete NaN`) con un `with` al lado que
        // no tiene nada que ver: sin el scope la divergencia sigue, el flag se va.
        let o4 = "var o = {};\nwith (o) { console.log(1); }\nconsole.log(delete (0 / 0));\n";
        let a4 = r.run(o4);
        assert_eq!(with_matters(&r, (Minifier::Oxc, Preset::CoreDefaults, SourceKind::Script), o4, o4, &a4, &a4), Recheck::NoExplica);
        // 02-with-positivo (c4): sin el scope el original devuelve otra cosa, y el
        // minificado ya se comporta asi: la asuncion explica la divergencia.
        let p = "function f() { var a = 1; with ({ a: 2 }) { return a; } }\nconsole.log(f());\n";
        let (ap, bp) = (r.run(p), r.run("console.log(1);\n"));
        assert_eq!(with_matters(&r, (Minifier::Oxc, Preset::CoreDefaults, SourceKind::Script), p, "console.log(1);\n", &ap, &bp), Recheck::Explica);
    }

    /// b7-1 (revision n2, IMPORTANTE 1), con node y oxc: el bug deja el valor por defecto y B
    /// == A' == B'. Antes Explica (known/); ahora el camino de B lo delata, en cada familia
    /// con neutralizacion: los stubs (q03), el `with` (q04), los built-ins (q06), la coercion
    /// (r07) y la TDZ atrapada (r01, por el sensor). Los positivos siguen en Explica.
    #[test]
    fn coincidencia_con_el_valor_por_defecto() {
        use super::{builtins_matter, coercion_matters};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("b71").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let cfg = Preset::CarrilA.build(Minifier::Oxc, SourceKind::Script);
        let ab = |src: &str| {
            let crate::minify::Outcome::Ok { original, minified, .. } = crate::minify::prepare(src, &cfg) else {
                panic!("no minifica: {src}");
            };
            let (a, b) = (r.run(&original), r.run(&minified));
            assert_eq!((a.stdout.as_str(), b.stdout.as_str()), ("\"bc\"\n", "\"\"\n"), "oxc ya no pliega mal substring: buscar otro bug\n{minified}");
            (original, minified, a, b)
        };
        let todavia = |x: Recheck| matches!(x, Recheck::Inconcluso(ref n) if n.contains("todavia pasa por la violacion"));
        let rc = (Minifier::Oxc, Preset::CarrilA, SourceKind::Script);
        let bug = "s = \"abc\".substring(1, undefined);";
        let (o, m, a, b) = ab(&format!("let s = \"\";\nif (typeof window === \"undefined\") {{ {bug} }}\nconsole.log(JSON.stringify(s));\n"));
        let g = globals_matter(&r, SourceKind::Script, &o, &m, &a, &b);
        assert!(todavia(g.clone()), "q03: {g:?}");
        let (o, m, a, b) = ab(&format!("var s = \"\";\nvar o = {{ k: 1 }};\nwith (o) {{ if (typeof k === \"number\") {bug} }}\nconsole.log(JSON.stringify(s));\n"));
        let w = with_matters(&r, rc, &o, &m, &a, &b);
        assert!(todavia(w.clone()), "q04: {w:?}");
        let (o, m, a, b) = ab(&format!("let s = \"\";\nArray.prototype.includes = function () {{ {bug} return false; }};\nif ([1].includes(2)) s = \"q\";\nconsole.log(JSON.stringify(s));\n"));
        let bi = builtins_matter(&r, &o, &m, &a, &b);
        assert!(todavia(bi.clone()), "q06: {bi:?}");
        let (o, m, a, b) = ab(&format!("let s = \"\";\nconst o = {{ valueOf() {{ {bug} return 1; }} }};\nif (o + 1 === 0) s = \"q\";\nconsole.log(JSON.stringify(s));\n"));
        let c = coercion_matters(&r, rc, &o, &m, &a, &b);
        assert!(todavia(c.clone()), "r07: {c:?}");
        let (o, m, a, b) = ab(&format!("let s = \"\";\ntry {{ if (String(x) === \"zz\") s = \"q\"; }} catch (e) {{ {bug} }}\nconsole.log(JSON.stringify(s));\nlet x = 1;\n"));
        let v = violaciones_matter(&r, rc, &o, &m, &a, &b, None);
        assert!(todavia(v.clone()), "r01: {v:?}");
        // 8.7: r01 con el try armado en runtime (`atob`) y un try muerto que oxc borra: A corre
        // con el sensor entero y B perezoso (`puede_atrapar` no ve nada en el minificado). B
        // perezoso no deja `viol`; antes Explica (known/), ahora el camino sale de B con el
        // sensor entero.
        let oculto = "(() => {}).constructor(\"f\", \"h\", atob(\"dHJ5IHsgZigpOyB9IGNhdGNoIChlKSB7IGgoKTsgfQ==\"))(f, h);";
        let (o, m, a, b) = ab(&format!("if (false) {{ try {{}} catch (e) {{}} }}\nlet s = \"\";\nconst f = () => {{ if (String(x) === \"zz\") s = \"q\"; }};\nconst h = () => {{ {bug} }};\n{oculto}\nconsole.log(JSON.stringify(s));\nlet x = 1;\n"));
        assert!(crate::runner::puede_atrapar(&o) && !crate::runner::puede_atrapar(&m), "{m}");
        assert!(!b.ctl.lines().any(|l| l.starts_with("viol ")) && a.flags.iter().any(|f| f == "tdz_atrapada"), "{}\n{}", a.ctl, b.ctl);
        let v = violaciones_matter(&r, rc, &o, &m, &a, &b, None);
        assert!(todavia(v.clone()), "8.7 p01: {v:?}");
        // Positivos (03-global-positivo y keep_comparisons): B ya no depende de la asuncion.
        let o = "try { window; console.log(\"hay window\"); } catch (e) { console.log(\"no hay\"); }\n";
        let m = "console.log(`hay window`);\n";
        assert_eq!(globals_matter(&r, SourceKind::Script, o, m, &r.run(o), &r.run(m)), Recheck::Explica);
        let o = "var o1 = { valueOf: function () { t1(); } };\nvar o2 = { valueOf: function () { t2(); } };\nvar r1 = o1 <= o2;\n";
        let m = "var o1 = { valueOf: function () { t1(); } };\nvar o2 = { valueOf: function () { t2(); } };\n";
        assert_eq!(coercion_matters(&r, rc, o, m, &r.run(o), &r.run(m)), Recheck::Explica);
    }

    /// b7-1b (revision 7.1, H1/H2), con node y oxc: B toma la asuncion como dato (un indice, una
    /// clave, lo que devuelve un helper nativo o un built-in pisado) o por una forma que la
    /// cobertura de V8 no separa (`&&=` que oxc arma de un `if`), sin cambiar de camino. Antes
    /// Explica (known/); ahora el sensor de uso lo ve. El texto de una funcion nativa ya no se
    /// fija (d05: NoExplica). Los positivos siguen en Explica.
    #[test]
    fn dependencia_por_datos() {
        use super::{builtins_matter, coercion_matters, contenido_matters};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("b71b").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let cfg = Preset::CarrilA.build(Minifier::Oxc, SourceKind::Script);
        let prep = |src: &str| {
            let crate::minify::Outcome::Ok { original, minified, .. } = crate::minify::prepare(src, &cfg) else {
                panic!("no minifica: {src}");
            };
            let (a, b) = (r.run(&original), r.run(&minified));
            (original, minified, a, b)
        };
        let ab = |src: &str| {
            let x = prep(src);
            assert_eq!((x.2.stdout.as_str(), x.3.stdout.as_str()), ("\"bc\"\n", "\"\"\n"), "oxc ya no pliega mal substring: buscar otro bug\n{}", x.1);
            x
        };
        let usa = |x: &Recheck, que: &str| matches!(x, Recheck::Inconcluso(ref n) if n.contains(&format!("B usa la asuncion: {que}")));
        let rc = (Minifier::Oxc, Preset::CarrilA, SourceKind::Script);
        let bug = "\"abc\".substring(1, undefined)";
        let (o, m, a, b) = ab(&format!("const t = [\"\", {bug}];\nconsole.log(JSON.stringify(t[+(typeof window === \"undefined\")]));\n"));
        let g = globals_matter(&r, SourceKind::Script, &o, &m, &a, &b);
        assert!(usa(&g, "global window"), "d01: {g:?}");
        let (o, m, a, b) = ab(&format!("let s = \"\";\nlet u = typeof window === \"undefined\";\nif (u) u = (s = {bug});\nconsole.log(JSON.stringify(s));\n"));
        assert!(m.contains("&&="), "g05b: oxc ya no arma `&&=`: {m}");
        let g = globals_matter(&r, SourceKind::Script, &o, &m, &a, &b);
        assert!(usa(&g, "global window"), "g05b: {g:?}");
        let (o, m, a, b) = prep(&format!("globalThis.k = \"\";\nvar o = {{ k: {bug} }};\nwith (o) {{ globalThis.s = k; }}\nconsole.log(JSON.stringify(globalThis.s));\n"));
        let w = with_matters(&r, rc, &o, &m, &a, &b);
        assert!(usa(&w, "with k"), "d02b: {w:?}");
        let (o, m, a, b) = ab(&format!("const o = {{ toString: Array.prototype.join, 0: \"x\", length: 1 }};\nconst t = {{ x: {bug}, \"[object Object]\": \"\" }};\nconsole.log(JSON.stringify(t[o]));\n"));
        let c = coercion_matters(&r, rc, &o, &m, &a, &b);
        assert!(usa(&c, "coercion toString"), "d03: {c:?}");
        let (o, m, a, b) = ab(&format!("Array.prototype.includes = Boolean;\nconst t = [\"\", {bug}];\nconsole.log(JSON.stringify(t[+[1].includes(2)]));\n"));
        let bi = builtins_matter(&r, &o, &m, &a, &b);
        assert!(usa(&bi, "builtins p2 includes"), "d04: {bi:?}");
        let (o, m, a, b) = ab(&format!("const t = [\"\", {bug}];\nconsole.log(JSON.stringify(t[+(String(Math.max) === \"function max() {{ [native code] }}\")]));\n"));
        let ct = contenido_matters(&r, &o, &m, &a, &b, &r.run_salted(&o));
        assert_eq!(ct, Recheck::NoExplica, "d05");
        // Positivos: 16-builtins-positivo (B ya no lee lo pisado), 17-tostring-positivo (el texto
        // de una funcion del programa sigue fijo) y el `with` de collapse_vars_eval_and_with (B ya
        // no resuelve nada en el objeto).
        let (o, m, a, b) = prep("String.prototype.toUpperCase = function () { return \"pisado\"; };\nconsole.log(\"abc\".toUpperCase());\n");
        assert_eq!(builtins_matter(&r, &o, &m, &a, &b), Recheck::Explica, "16-builtins-positivo: {m}");
        let (o, m, a, b) = prep("function suma(primero, segundo) { return primero + segundo; }\nconsole.log(String(suma));\n");
        assert_eq!(contenido_matters(&r, &o, &m, &a, &b, &r.run_salted(&o)), Recheck::Explica, "17-tostring-positivo: {m}");
        let (o, m, a, b) = prep("(function f1() { var o = { a: 1 }, a = 2; with (o) console.log(a); })();\n");
        assert!(m.contains("with"), "{m}");
        assert_eq!(with_matters(&r, rc, &o, &m, &a, &b), Recheck::Explica, "collapse_vars_eval_and_with: {m}");
    }

    /// b7-1c (revision 7.1b): P1, el texto de una funcion con contadores es el de B (17-tostring con
    /// un valor por defecto o un `||=` sigue Explica; el bug en un valor por defecto elegido por el
    /// texto no); S1, si el salado no diverge, la sal cambio el camino (Inconcluso); S2, los ganchos
    /// solo con DIFUZZ_GANCHOS=1 y la cobertura no se vacia si el programa pisa lo que usa el preload.
    #[test]
    fn texto_de_funciones_sal_y_preload() {
        use super::{con_literales_salados, contenido_matters, GANCHOS};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("b71c").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let cfg = Preset::CarrilA.build(Minifier::Oxc, SourceKind::Script);
        let prep = |src: &str| {
            let crate::minify::Outcome::Ok { original, minified, .. } = crate::minify::prepare(src, &cfg) else {
                panic!("no minifica: {src}");
            };
            let (a, b) = (r.run(&original), r.run(&minified));
            (original, minified, a, b)
        };
        let contenido = |o: &str, m: &str, a: &Exec, b: &Exec| contenido_matters(&r, o, m, a, b, &r.run_salted(o));
        // P1: con el contador insertado (no re-emitido) y sacado en `toString`, B da lo mismo.
        for src in [
            "function suma(primero, segundo = 1) { return primero + segundo; }\nconsole.log(String(suma));\n",
            "function f(o) { o.x ||= 1; return o.x; }\nconsole.log(String(f));\n",
        ] {
            let (o, m, a, b) = prep(src);
            assert_ne!(a.stdout, b.stdout, "{m}");
            assert_eq!(contenido(&o, &m, &a, &b), Recheck::Explica, "{src}{m}");
        }
        let (o, m, a, b) = prep("let s = \"\";\nfunction f0() { return 1; }\nfunction h(p = (s = \"abc\".substring(1, undefined))) { return p; }\nh([undefined, 0][+((String(f0).indexOf(\"return\") >= 0) + (String(f0).indexOf(\"/*\") < 0) !== 2)]);\nconsole.log(JSON.stringify(s));\n");
        assert_eq!((a.stdout.as_str(), b.stdout.as_str()), ("\"bc\"\n", "\"\"\n"), "{m}");
        let c = contenido(&o, &m, &a, &b);
        assert!(matches!(c, Recheck::Inconcluso(ref n) if n.contains("cobertura")), "p1n: {c:?}");
        // S1 (x5b): un literal guarda la rama del bug; el salado no diverge.
        let src = "function f0() { return 1; }\nconst modo = \"x\";\nconst t = [\"\", modo === \"x\" ? \"abc\".substring(1, undefined) : \"bc\"];\nconsole.log(JSON.stringify(t[+((String(f0).indexOf(\"return\") >= 0) + (String(f0).indexOf(\"/*\") < 0) === 2)]));\n";
        let (o, m, a, b) = prep(src);
        assert_eq!((a.stdout.as_str(), b.stdout.as_str()), ("\"bc\"\n", "\"\"\n"), "{m}");
        assert_eq!(contenido(&o, &m, &a, &b), Recheck::Explica, "x5b sin la sal");
        let s = con_literales_salados(&r, (Minifier::Oxc, Preset::CarrilA, SourceKind::Script), &o, super::Sal::Dato, |o2, m2, a2, b2| contenido(o2, m2, a2, b2));
        assert!(matches!(s, Err(ref n) if n.contains("la sal cambio el camino")), "x5b: {s:?}");
        // S2: los ganchos solo con el texto instrumentado.
        let g = "console.log(typeof __difuzzNeutroRama, typeof __difuzzNeutroThis);\n";
        assert_eq!(r.run_con_cobertura(g, &[], &[]).stdout, "undefined undefined\n");
        assert_eq!(r.run_con_cobertura(g, &[], &[GANCHOS]).stdout, "function function\n");
        // S2: lo que el programa pisa al final no vacia ni aplana la cobertura (rev-7.1b x8, x8b).
        for pisa in [
            "String.prototype.startsWith = function () { return true; };",
            "Object.getPrototypeOf([][Symbol.iterator]()).next = function () { return { done: true }; };",
            "Math.imul = function () { return 0; };",
            "String.prototype.charCodeAt = function () { return 0; };",
            "Number.prototype.toString = function () { return \"0\"; };",
            "require(\"inspector\").Session.prototype.post = function (m, p, cb) { (cb || p)(null, { result: [] }); };",
        ] {
            let cov = |n: u32| r.run_con_cobertura(&format!("let s = 0;\nfor (let i = 0; i < {n}; i++) s++;\nconsole.log(s > 0);\n{pisa}\n"), &[], &[]).cobertura().map(String::from);
            let (x, y) = (cov(2), cov(3));
            assert!(x.is_some() && y.is_some() && x != y, "{pisa}: {x:?} {y:?}");
            assert!(!x.unwrap().ends_with(" 0"), "{pisa}: sin rangos");
        }
    }

    /// b4-a: la posicion de V8 (0-based, UTF-16) a byte, y la neutralizacion por clase.
    #[test]
    fn neutraliza_violaciones() {
        assert_eq!(offset_utf16("ab\ncd", 1, 1), Some(4));
        assert_eq!(offset_utf16("a\r\nb", 1, 0), Some(3));
        // "ñ" es 2 bytes y 1 unidad UTF-16; el emoji, 4 bytes y 2 unidades.
        assert_eq!(offset_utf16("\"ñ😀\";x", 0, 5), Some(8));
        assert_eq!(offset_utf16("ab", 0, 3), None);
        assert_eq!(offset_utf16("ab\ncd", 0, 3), None);
        let k = SourceKind::Script;
        // TDZ: el identificador que empieza en la posicion.
        let out = neutralize_violaciones("try { x; } catch (e) {}\nlet x = 1;\n", k, "tdz 0:6").unwrap();
        assert!(out.contains("void 0") && out.contains("let x = 1"), "{out}");
        // Symbol: V8 da la posicion de `abs`; se cambia la llamada entera.
        let out = neutralize_violaciones("try { Math.abs(Symbol()); } catch (e) {}\n", k, "symbol 0:11").unwrap();
        assert!(out.contains("void 0") && !out.contains("Math.abs"), "{out}");
        // extends: el heredero que contiene la posicion pasa a null; el otro no.
        let src = "const v = [];\nclass B {}\ntry { class A extends v {} } catch (e) {}\nclass C extends B {}\n";
        let out = neutralize_violaciones(src, k, "extends 2:22").unwrap();
        assert!(out.contains("extends null") && out.contains("extends B"), "{out}");
        // Una que no se ubica (0:0 de un heredero literal) o sin posicion: None.
        assert!(neutralize_violaciones("try { class A extends [] {} } catch (e) {}\n", k, "extends 0:0").is_none());
        assert!(neutralize_violaciones("try { x; } catch (e) {}\nlet x;\n", k, "tdz 0:6\nsymbol -").is_none());
        assert!(neutralize_violaciones("x;\n", k, "tdz 5:0").is_none());
        // n-principio (Fase R I2): V8 da la columna del `return`/`throw`. Un solo candidato
        // (ligado con let/const/class): solo el pasa a `void 0`; el resto de la sentencia queda.
        let out = neutralize_violaciones("function g() { return x; }\nlet x = 1;\n", k, "tdz 0:15").unwrap();
        assert!(out.contains("return void 0") && out.contains("let x = 1"), "{out}");
        let out = neutralize_violaciones("function g() { throw x; }\nconst x = 1;\n", k, "tdz 0:15").unwrap();
        assert!(out.contains("throw void 0"), "{out}");
        // n16: el bug en la misma sentencia sigue (Math y log no son candidatos).
        let out = neutralize_violaciones("function log(v) { return 0; }\nfunction g() { return log(Math.cbrt(-8)) + x; }\nlet x = 1;\n", k, "tdz 1:15").unwrap();
        assert!(out.contains("log(Math.cbrt(-8)) + void 0"), "{out}");
        // Dos candidatos (o el mismo dos veces), ninguno, o lo unico esta en una flecha: no se ubica.
        assert!(neutralize_violaciones("function g() { return x + y; }\nlet x, y;\n", k, "tdz 0:15").is_none());
        assert!(neutralize_violaciones("function g() { return x + x; }\nlet x;\n", k, "tdz 0:15").is_none());
        assert!(neutralize_violaciones("function g() { return 1; }\nlet x;\n", k, "tdz 0:15").is_none());
        assert!(neutralize_violaciones("function g() { return () => x; }\nlet x;\n", k, "tdz 0:15").is_none());
        // Un `var` no tiene TDZ: no es candidato.
        assert!(neutralize_violaciones("var y;\nfunction g() { return y; }\n", k, "tdz 1:15").is_none());
    }

    /// b4-a (R1-H04 / C3): el sensor ve las violaciones atrapadas, y el re-chequeo
    /// solo las explica si la divergencia es esa. Al lado de un bug real de oxc
    /// (O4, #24331: `delete (0/0)` -> `delete NaN`) la divergencia sigue: div/.
    #[test]
    fn violaciones_atrapadas_en_node() {
        use crate::minify::{self, SourceKind};
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("viol").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let cfg = Preset::CoreDefaults.build(Minifier::Oxc, SourceKind::Script);
        let caso = |src: &str| {
            let minify::Outcome::Ok { original, minified, .. } = minify::prepare(src, &cfg) else {
                panic!("no minifica: {src}");
            };
            let (a, b) = (r.run(&original), r.run(&minified));
            let rc = violaciones_matter(&r, (Minifier::Oxc, Preset::CoreDefaults, SourceKind::Script), &original, &minified, &a, &b, None);
            (a, b, rc)
        };
        let bug = "console.log(delete (0 / 0));\n";
        for (prog, flag) in [
            ("try { x; console.log(\"sin tdz\"); } catch (e) { console.log(\"tdz\"); }\nlet x = 1;\n", "tdz_atrapada"),
            ("try { Math.abs(Symbol()); console.log(\"no lanzo\"); } catch (e) { console.log(\"lanzo\"); }\n", "coercion_symbol"),
            ("const v = [];\ntry { class A extends v {} console.log(\"no lanzo\"); } catch (e) { console.log(\"lanzo\"); }\n", "extends_atrapado"),
        ] {
            let (a, b, rc) = caso(prog);
            assert!(a.flags.iter().any(|f| f == flag), "{prog}: {:?}", a.flags);
            assert_ne!(a.stdout, b.stdout, "oxc ya no explota la asuncion: {prog}");
            assert_eq!(rc, Recheck::Explica, "{prog}");
            // Negativo: el mismo uso inocuo y un bug real al lado.
            let (a, b, rc) = caso(&format!("{prog}{bug}"));
            assert!(a.flags.iter().any(|f| f == flag), "{prog}: {:?}", a.flags);
            assert!(b.stdout.ends_with("false\n") && a.stdout.ends_with("true\n"), "oxc ya no tiene O4: {}", b.stdout);
            assert_eq!(rc, Recheck::NoExplica, "{prog}{bug}");
        }
        // n-principio (n10, n17, n06): el bug en el catch de la violacion, que en A' no corre
        // (A=-2, B=NaN, A'=undefined o "function"). Con `veredicto` (`localizada`) iban a known/.
        for prog in [
            "let r = \"none\";\ntry { r = String(x); } catch (e) { r = Math.cbrt(-8); }\nconsole.log(r);\nlet x = 1;\n",
            "let r = \"none\";\nvar s = Symbol(\"s\");\ntry { r = Math.abs(s); } catch (e) { r = Math.cbrt(-8); }\nconsole.log(r);\n",
            "let r = \"none\";\nvar v = [];\ntry { class A extends v {} r = typeof A; } catch (e) { r = Math.cbrt(-8); }\nconsole.log(r);\n",
        ] {
            let (a, b, rc) = caso(prog);
            assert_eq!((a.stdout.as_str(), b.stdout.as_str()), ("-2\n", "NaN\n"), "oxc ya no pliega mal Math.cbrt(-8): {prog}");
            assert!(matches!(rc, Recheck::Inconcluso(ref n) if n.contains("B != A'")), "{prog}: {rc:?}");
        }
        // n-principio (Fase R I2): la TDZ en un `return` (V8 la ubica en la sentencia). Antes
        // inconcluso por "no pude ubicar"; ahora se neutraliza el unico candidato.
        // - Positivo: oxc borra la llamada (asume que no hay TDZ) y B se comporta como A'.
        // - n15 (el bug en el catch): B != A', inconcluso. n16 (el bug en la misma sentencia,
        //   antes del identificador): el bug sigue en A' y B', NoExplica.
        let (a, b, rc) = caso("function g() { return x; }\ntry { g(); console.log(\"sin tdz\"); } catch (e) { console.log(\"tdz\"); }\nlet x = 1;\n");
        assert_ne!(a.stdout, b.stdout);
        assert_eq!(rc, Recheck::Explica, "{} | {}", a.stdout, b.stdout);
        let (_, _, rc) = caso("let r = \"none\";\nfunction g() { return x; }\ntry { r = g(); } catch (e) { r = Math.cbrt(-8); }\nconsole.log(r);\nlet x = 1;\n");
        assert!(matches!(rc, Recheck::Inconcluso(ref n) if n.contains("B != A'")), "{rc:?}");
        let (_, _, rc) = caso("let r = \"none\";\nfunction log(v) { console.log(v); return 0; }\nfunction g() { return log(Math.cbrt(-8)) + x; }\ntry { r = g(); } catch (e) { r = \"tdz\"; }\nconsole.log(r);\nlet x = 1;\n");
        assert_eq!(rc, Recheck::NoExplica);
        // Sin violacion ubicada (coercion de Symbol no atrapada sin marcos) no hay
        // re-chequeo posible: inconcluso.
        let a = r.run("console.log(1);\n");
        let rc = violaciones_matter(&r, (Minifier::Oxc, Preset::CoreDefaults, SourceKind::Script), "console.log(1);\n", "console.log(1);\n", &a, &a, None);
        assert!(matches!(rc, Recheck::Inconcluso(_)), "{rc:?}");
    }

    /// b4-a: el sensor solo observa. La salida no cambia; lo que atrapa el preload
    /// (invocar, leer un export en TDZ) no cuenta como atrapado por el programa; la
    /// coercion de Symbol no atrapada se ubica igual.
    #[test]
    fn sensor_de_violaciones_solo_observa() {
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("viol2").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let e = r.run("let n = 0;\nfor (let i = 0; i < 300; i++) { try { y; } catch (e) { n++; } }\nconsole.log(n);\nlet y;\n");
        assert_eq!(e.stdout, "300\n");
        assert!(e.flags.iter().any(|f| f == "tdz_atrapada"), "{:?}", e.flags);
        assert_eq!(super::violaciones(&e), ["tdz 1:38"], "{}", e.ctl);
        // No atrapada: `tdz` como siempre, sin `tdz_atrapada`. I2: ahora con su posicion
        // (para el re-chequeo de violaciones), tambien sin el sensor del inspector.
        let e = r.run("x;\nlet x;\n");
        assert!(e.flags == ["tdz"], "{:?}", e.flags);
        assert_eq!(super::violaciones(&e), ["tdz 0:0"], "{}", e.ctl);
        let e = r.sin_sensor().run("console.log(1);\n  x;\nlet x;\n");
        assert!(e.flags == ["tdz"], "{:?}", e.flags);
        assert_eq!(super::violaciones(&e), ["tdz 1:2"], "{}", e.ctl);
        // Symbol no atrapado: flag y posicion por el primer frame.
        let e = r.run("console.log(1);\nMath.abs(Symbol());\n");
        assert!(e.threw() && e.flags.iter().any(|f| f == "coercion_symbol"), "{:?}", e.flags);
        assert_eq!(super::violaciones(&e), ["symbol 1:5"], "{}", e.ctl);
        // Node muerto por una senal (process.pid es falso: abort): se repite sin el
        // sensor (runner::run_inner) y el canal lo dice. El status es el de siempre.
        let e = r.run("console.log(1);\nprocess.abort();\n");
        assert_eq!(e.status, crate::runner::Status::Signal);
        assert!(e.ctl.starts_with("sensor_caido\n"), "{}", e.ctl);
        // Una recursion que atrapa en cada nivel: con el sensor no termina a tiempo
        // (cada throw recorre la pila); se repite sin el y da lo de siempre.
        let rec = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(4));
        let e = rec.run("function foo(){try{foo()}catch(ex){let x=10;const y=10;class Zee{}}console.log(x,y,Zee)};try{foo()}catch(e){console.log(\"fin\")}\n");
        assert_eq!((e.status.clone(), e.stdout.as_str()), (crate::runner::Status::Exited(0), "fin\n"));
        assert!(e.ctl.starts_with("sensor_caido\n"), "{}", e.ctl);
        // `debugger;` no pausa (si no, un loop con `debugger;` se vuelve lentisimo).
        let t = std::time::Instant::now();
        let e = r.run("let n = 0;\nfor (let i = 0; i < 20000; i++) { debugger; n++; }\nconsole.log(n);\n");
        assert_eq!(e.stdout, "20000\n");
        assert!(t.elapsed() < std::time::Duration::from_secs(4), "{:?}", t.elapsed());
        // invocar: la TDZ la atrapa el preload (no el programa): cuenta como `tdz`.
        let e = r.run("function f() { return z; }\n__difuzzObserveInvocar([\"f\", f]);\nlet z = 1;\n");
        assert!(e.stdout.contains("f() throws ReferenceError"), "{}", e.stdout);
        assert!(e.flags.iter().any(|f| f == "tdz") && !e.flags.iter().any(|f| f == "tdz_atrapada"), "{:?}", e.flags);
        // V8 ubica una TDZ dentro de `return`/`throw` en la sentencia (MEDIDO, node 24), no
        // en el identificador. n-principio: el neutralizador la busca en la sentencia si hay
        // un solo candidato (neutraliza_violaciones); si no, inconcluso (div/).
        assert_eq!(super::violaciones(&e), ["tdz 0:15"], "{}", e.ctl);
        // Carril B: el programa es el modulo que importa el envoltorio.
        let m = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20)).with_kind(SourceKind::Module);
        let e = m.run("try { x; } catch {}\nexport let x = 1;\n");
        assert_eq!(super::violaciones(&e), ["tdz 0:6"], "{}", e.ctl);
        // ... y una no atrapada: V8 pausa como "atrapada" (el rechazo del import lo
        // maneja node), pero onThrow la reconoce y queda solo `tdz`.
        let e = m.run("x;\nexport let x;\n");
        assert!(e.flags.iter().any(|f| f == "tdz") && !e.flags.iter().any(|f| f == "tdz_atrapada"), "{:?}", e.flags);
        assert_eq!(super::violaciones(&e), ["tdz 0:0"], "{}", e.ctl);
        // I3 (revision-b456): tras MAX_PAUSAS (256) el sensor se apaga y el canal lo dice.
        let e = r.run("let n = 0;\nfor (let i = 0; i < 300; i++) { try { null.x; } catch (e) { n++; } }\nconsole.log(n);\n");
        assert_eq!(e.stdout, "300\n");
        assert_eq!(e.sensor_degradado(), Some(crate::runner::SENSOR_TOPE), "{}", e.ctl);
        let e = r.run("console.log(1);\n");
        assert_eq!(e.sensor_degradado(), None, "{}", e.ctl);
    }
}

/// 8.3 (= 7.11): el re-chequeo `mensaje_nombre`.
#[cfg(test)]
mod mensaje_nombre_tests {
    use super::{alfa_equivalentes, linea_con_nombre, mensaje_con_nombre_localizado, mensaje_nombre_con, mensaje_nombre_matters, Recheck};
    use crate::backend::Minifier;
    use crate::minify::{prepare, Outcome, Preset, SourceKind};
    use crate::runner::{Exec, Preload, Runner, Status};

    #[test]
    fn linea_con_nombre_casos() {
        assert!(linea_con_nombre("[error] TypeError: cb is not a function", "[error] TypeError: e is not a function"));
        assert!(linea_con_nombre("TypeError: o.cb is not a function", "TypeError: e.cb is not a function"));
        assert!(linea_con_nombre("x is not iterable", "t is not iterable"));
        assert!(linea_con_nombre("TypeError: Foo is not a constructor", "TypeError: n is not a constructor"));
        assert!(!linea_con_nombre("TypeError: cb is not a function", "TypeError: cb is not a function"));
        assert!(!linea_con_nombre("[error] TypeError: cb is not a function", "[log] TypeError: e is not a function"));
        // Un global no se manglea: `is not defined` queda afuera.
        assert!(!linea_con_nombre("ReferenceError: foo is not defined", "ReferenceError: a is not defined"));
        assert!(!linea_con_nombre("TypeError: cb is not a function 1", "TypeError: e is not a function 2"));
        assert!(!linea_con_nombre("1 is not a function", "2 is not a function"));
        assert!(!linea_con_nombre("TypeError: cb is not a function", "TypeError: e is not a constructor"));
        assert!(!linea_con_nombre("TypeError: a..b is not a function", "TypeError: e is not a function"));
        // Un nombre privado, como lo escribe V8.
        assert!(linea_con_nombre("this[#handler] is not a function 1", "this[#e] is not a function 1"));
        // Un marco de stack con otro nombre de funcion (core_defaults) no entra (n-principio).
        assert!(!linea_con_nombre("    at run (<pos>)", "    at e (<pos>)"));
    }

    /// 9.3 (M3): si el hijo sin mangle pasa el presupuesto de memoria del worker o no termina a
    /// tiempo, el re-chequeo queda `Inconcluso` con la causa (div/ con nota), no Explica. El
    /// hijo es el de los tests de subproceso.rs (`remoto_de_test_con`): come 300 MiB contra un
    /// presupuesto de 128 MiB, o duerme mas que su timeout.
    #[test]
    fn hijo_sin_mangle_sin_recursos_es_inconcluso() {
        if !std::path::Path::new("/proc/self/statm").exists() {
            return;
        }
        use crate::subproceso::tests::remoto_de_test_con;
        use std::time::Duration;
        let preload = Preload::write_tagged("mnombre-m3").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20));
        let (a, b) = (ex("cb is not a function\n"), ex("e is not a function\n"));
        let src = "function run(cb){try{cb()}catch(e){console.log(e.message)}}run();\n";
        let min = "function run(e){try{e()}catch(t){console.log(t.message)}}run();\n";
        let s = SourceKind::Script;
        let come = [("DIFUZZ_TEST_COMER_MIB", "300"), ("DIFUZZ_TEST_DORMIR_MS", "20000")];
        let cfg = crate::minify::Config::from_backend(Box::new(remoto_de_test_con(Minifier::Oxc, s, &come, Duration::from_secs(30), Some(128 << 20))));
        let rc = super::mensaje_nombre_desde(&r, &cfg, (s, false), src, min, &a, &b);
        assert!(matches!(rc, Recheck::Inconcluso(ref n) if n.contains(crate::subproceso::PREFIJO_MEMORIA)), "{rc:?}");
        let duerme = [("DIFUZZ_TEST_DORMIR_MS", "20000")];
        let cfg = crate::minify::Config::from_backend(Box::new(remoto_de_test_con(Minifier::Oxc, s, &duerme, Duration::from_millis(300), Some(0))));
        let rc = super::mensaje_nombre_desde(&r, &cfg, (s, false), src, min, &a, &b);
        assert!(matches!(rc, Recheck::Inconcluso(ref n) if n.contains(crate::subproceso::PREFIJO_TIMEOUT)), "{rc:?}");
    }

    /// 9.3 (M2): las llamadas (`(...)`) y los accesos opcionales (`?.`) como los escribe V8
    /// (MEDIDO en node 24.14.1: sondas/m2-v8.cjs y m2-v8-b.cjs de 9.3).
    #[test]
    fn linea_con_nombre_llamadas_v8() {
        let si = |x: &str, y: &str| assert!(linea_con_nombre(x, y), "{x} | {y}");
        let no = |x: &str, y: &str| assert!(!linea_con_nombre(x, y), "{x} | {y}");
        // c03 de rev-b8-A: `f()()`, con o sin argumentos.
        si("TypeError: f(...) is not a function", "TypeError: t(...) is not a function");
        si("f(...)(...) is not a function", "e(...)(...) is not a function");
        si("o.f(...) is not a function", "e.f(...) is not a function");
        si("TypeError: f(...).g is not a function", "TypeError: e(...).g is not a function");
        si("f(...).g(...) is not a function", "e(...).g(...) is not a function");
        si("this.m(...)[#h] is not a function", "this.m(...)[#e] is not a function");
        si("o[f(...)] is not a function", "o[e(...)] is not a function");
        si("f(...)[0] is not a function", "e(...)[0] is not a function");
        si("f(...) is not a constructor", "e(...) is not a constructor");
        // Opcionales.
        si("o?.g is not a function", "e?.g is not a function");
        si("o.a?.b is not a function", "e.a?.b is not a function");
        si("o?.[k] is not a function", "e?.[t] is not a function");
        si("f(...)?.g is not a function", "e(...)?.g is not a function");
        // Lo que V8 no escribe, o que no es una expresion de nombres, sigue afuera.
        no("(...) is not a function", "e is not a function");
        no("f(...) is not a function", "f(...) is not a function");
        no("x.(...) is not a function", "e.(...) is not a function");
        no("o[(...)] is not a function", "e[(...)] is not a function");
        no("f(1) is not a function", "e(1) is not a function");
        no("f() is not a function", "e() is not a function");
        no("f(..) is not a function", "e(..) is not a function");
        no("o? is not a function", "e? is not a function");
        no("o?. is not a function", "e?. is not a function");
        no("f!g is not a function", "e!g is not a function");
        no("(intermediate value) is not a constructor", "(intermediate value) is not a constructor");
        no("(0 , f) is not a function", "(0 , e) is not a function");
        // El resto de la linea (antes y despues) tiene que ser el mismo.
        no("[log] f(...) is not a function", "[error] e(...) is not a function");
        no("TypeError: f(...) is not a function", "TypeError: e(...) is not a constructor");
    }

    fn ex(stdout: &str) -> Exec {
        Exec { status: Status::Exited(0), stdout: stdout.into(), stderr: String::new(), flags: Vec::new(), elapsed: std::time::Duration::ZERO, ctl: String::new() }
    }

    #[test]
    fn localizacion() {
        let a = ex("antes\n[error] TypeError: cb is not a function\nfin\n");
        assert!(mensaje_con_nombre_localizado(&a, &ex("antes\n[error] TypeError: e is not a function\nfin\n")));
        // Otra linea distinta (un bug al lado): no.
        assert!(!mensaje_con_nombre_localizado(&a, &ex("antes\n[error] TypeError: e is not a function\nFIN\n")));
        // Una linea de mas o de menos: no.
        assert!(!mensaje_con_nombre_localizado(&a, &ex("antes\n[error] TypeError: e is not a function\n")));
        // Iguales: no hay nada que explicar.
        assert!(!mensaje_con_nombre_localizado(&a, &a));
        // Distinto estado de salida: no.
        let mut b = ex("antes\n[error] TypeError: e is not a function\nfin\n");
        b.status = Status::Exited(1);
        assert!(!mensaje_con_nombre_localizado(&a, &b));
    }

    #[test]
    fn alfa_equivalencia() {
        let s = SourceKind::Script;
        let ok = |x: &str, y: &str, k| assert_eq!(alfa_equivalentes(x, y, k), Some(Ok(Vec::new())), "{x} | {y}");
        let no = |x: &str, y: &str, k| assert!(matches!(alfa_equivalentes(x, y, k), Some(Err(_))), "{x} | {y}: {:?}", alfa_equivalentes(x, y, k));
        let b0 = "function run(cb){var x=1;try{cb()}catch(e){console.log(e.message,x)}}run();";
        ok(b0, "function run(t){var n=1;try{t()}catch(r){console.log(r.message,n)}}run();", s);
        // El riesgo de propuesta-1: el mangle hace que la llamada capture OTRO binding no
        // llamable (`n`, que vale 1): el mensaje tendria la misma forma.
        no(b0, "function run(t){var n=1;try{n()}catch(r){console.log(r.message,n)}}run();", s);
        // Dos bindings fundidos en un nombre en el mismo scope.
        no(b0, "function run(t){var t=1;try{t()}catch(r){console.log(r.message,t)}}run();", s);
        // Un local que pasa a llamarse como un global que se usa (`console` capturado).
        no("function f(a){return console.log(a)}", "function f(console){return console.log(console)}", s);
        // Un global renombrado.
        no("foo();", "e();", s);
        // Un local renombrado que captura una referencia de un scope de afuera.
        no("function f(a){var b=1;return function(){return a}}", "function f(e){var t=1;return function(){return t}}", s);
        // Nombres reusados en scopes hermanos: valido.
        ok("function f(a){return a}function g(b){return b}", "function f(e){return e}function g(e){return e}", s);
        // Formas abreviadas que el mangle expande.
        ok("function f(cb){return {cb}}", "function f(e){return {cb:e}}", s);
        no("function f(cb){return {cb}}", "function f(e){return {cx:e}}", s);
        ok("function f({cb}){return cb}", "function f({cb:e}){return e}", s);
        ok("function f({cb=1}){return cb}", "function f({cb:e=1}){return e}", s);
        // Nombres privados (renombrados por clase).
        ok("class A{#x=1;g(){return this.#x}}", "class A{#a=1;g(){return this.#a}}", s);
        no("class A{#x=1;#y=2;g(){return this.#x}}", "class A{#a=1;#b=2;g(){return this.#b}}", s);
        // Import/export (modulo): el nombre exportado no cambia.
        let m = SourceKind::Module;
        ok("let x=1;export{x}", "let e=1;export{e as x}", m);
        no("let x=1;export{x}", "let e=1;export{e as y}", m);
        ok("import{x}from'm';console.log(x)", "import{x as e}from'm';console.log(e)", m);
        // `eval` directo y `with`: la resolucion es dinamica, no se puede afirmar.
        no("function f(cb){eval('cb()')}", "function f(e){eval('cb()')}", s);
        no("function f(cb){with({})cb()}", "function f(e){with({})e()}", s);
        // 9.3 (M4): con parentesis el eval sigue siendo directo, y un binding local que se
        // llama `eval` puede valer %eval% (directo tambien; MEDIDO en node). `(0, eval)` y
        // `eval?.()` son indirectos: ven solo globales, el renombre de un local no los toca.
        no("function f(cb){(eval)('cb()')}", "function f(e){(eval)('cb()')}", s);
        no("function f(cb){((eval))('cb()')}", "function f(e){((eval))('cb()')}", s);
        no("function f(eval,cb){eval('cb()')}", "function f(t,e){t('cb()')}", s);
        no("function f(cb){var eval=globalThis.eval;eval('cb()')}", "function f(e){var t=globalThis.eval;t('cb()')}", s);
        ok("function f(cb){(0,eval)('1');return cb}", "function f(e){(0,eval)('1');return e}", s);
        ok("function f(cb){eval?.('1');return cb}", "function f(e){eval?.('1');return e}", s);
        // No parsea.
        assert_eq!(alfa_equivalentes("function(", "x", s), None);
    }

    /// H1 (revision del bloque 8): los renombres de bindings de los que sale un `.name`
    /// (`Nombran`) salen de la alfa-equivalencia, y solo esos.
    #[test]
    fn renombres_que_cambian_el_name() {
        let (s, m) = (SourceKind::Script, SourceKind::Module);
        let r = |x: &str, y: &str, k| match alfa_equivalentes(x, y, k) {
            Some(Ok(v)) => v,
            otro => panic!("{x} | {y}: {otro:?}"),
        };
        // Los dos repros de la revision (b01, b02): default de parametro anonimo.
        assert_eq!(r("function run(p){const check=(handler=()=>{})=>{console.log(handler.name)};check();return p}", "function run(p){const check=(e=()=>{})=>{console.log(e.name)};check();return p}", s), ["handler -> e"]);
        assert_eq!(r("function crear(Base=class{}){return Base.name}", "function crear(e=class{}){return e.name}", s), ["Base -> e"]);
        // Un parametro sin default anonimo: un renombre que no toca ningun `.name`.
        assert!(r("function run(cb){cb()}", "function run(e){e()}", s).is_empty());
        assert!(r("function f(h=1){return h}", "function f(e=1){return e}", s).is_empty());
        // Un default con nombre propio: el `.name` es el de la funcion, no el del parametro.
        assert!(r("function f(h=function g(){}){return h}", "function f(e=function g(){}){return e}", s).is_empty());
        // ...pero renombrar el nombre propio si lo cambia.
        assert_eq!(r("function f(h=function g(){}){return h}", "function f(h=function e(){}){return h}", s), ["g -> e"]);
        // Declarador, con parentesis, `async` y generador.
        assert_eq!(r("function f(){let h=(async()=>{});return h}", "function f(){let e=(async()=>{});return e}", s), ["h -> e"]);
        assert_eq!(r("function f(){var h=function*(){};return h}", "function f(){var e=function*(){};return e}", s), ["h -> e"]);
        // Destructuring: `{h = ...}` (expandido), `{k: h = ...}`, `[h = ...]`, en parametros y en
        // declaradores.
        assert_eq!(r("function f({h=()=>{}}){return h}", "function f({h:e=()=>{}}){return e}", s), ["h -> e"]);
        assert_eq!(r("function f({k:h=()=>{}}){return h}", "function f({k:e=()=>{}}){return e}", s), ["h -> e"]);
        assert_eq!(r("function f(){const[h=class{}]=[];return h}", "function f(){const[e=class{}]=[];return e}", s), ["h -> e"]);
        // Asignaciones: `=`, logicas y por destructuring; `+=` no hace NamedEvaluation.
        assert_eq!(r("function f(){let h;h=()=>{};return h}", "function f(){let e;e=()=>{};return e}", s), ["h -> e"]);
        assert_eq!(r("function f(){let h;h||=()=>{};return h}", "function f(){let e;e||=()=>{};return e}", s), ["h -> e"]);
        assert_eq!(r("function f(){let h;h??=class{};return h}", "function f(){let e;e??=class{};return e}", s), ["h -> e"]);
        assert_eq!(r("function f(){let h;[h=()=>{}]=[];return h}", "function f(){let e;[e=()=>{}]=[];return e}", s), ["h -> e"]);
        assert_eq!(r("function f(){let h;({h=()=>{}}={});return h}", "function f(){let e;({h:e=()=>{}}={});return e}", s), ["h -> e"]);
        assert!(r("function f(){let h=1;h+=()=>{};return h}", "function f(){let e=1;e+=()=>{};return e}", s).is_empty());
        // 9.3 (M5): con el destino entre parentesis no hay NamedEvaluation (`.name` "", MEDIDO
        // en node 24.14.1: sondas/m5-name.cjs de 9.3): no se marca.
        assert!(r("function f(){let h;(h)=()=>{};return h}", "function f(){let e;(e)=()=>{};return e}", s).is_empty());
        assert!(r("function f(){let h;((h))=class{};return h}", "function f(){let e;((e))=class{};return e}", s).is_empty());
        assert!(r("function f(){let h;(h)||=()=>{};return h}", "function f(){let e;(e)||=()=>{};return e}", s).is_empty());
        assert!(r("function f(){let h;[(h)=()=>{}]=[];return h}", "function f(){let e;[(e)=()=>{}]=[];return e}", s).is_empty());
        assert!(r("function f(){let h;({k:(h)=()=>{}}={});return h}", "function f(){let e;({k:(e)=()=>{}}={});return e}", s).is_empty());
        assert!(r("function f(){let h;for([(h)=()=>{}]of[[]]);return h}", "function f(){let e;for([(e)=()=>{}]of[[]]);return e}", s).is_empty());
        // ...y sin parentesis en el destino sigue marcado, con parentesis en el valor tambien.
        assert_eq!(r("function f(){let h;h=(()=>{});return h}", "function f(){let e;e=(()=>{});return e}", s), ["h -> e"]);
        assert_eq!(r("function f(){let h;({k:h=()=>{}}={});return h}", "function f(){let e;({k:e=()=>{}}={});return e}", s), ["h -> e"]);
        assert_eq!(r("function f(){let h;for([h=()=>{}]of[[]]);return h}", "function f(){let e;for([e=()=>{}]of[[]]);return e}", s), ["h -> e"]);
        assert_eq!(r("function f(h=(()=>{})){return h}", "function f(e=(()=>{})){return e}", s), ["h -> e"]);
        // Una expresion que no es una definicion anonima: `(0, () => {})` no da nombre.
        assert!(r("function f(){let h=(0,()=>{});return h}", "function f(){let e=(0,()=>{});return e}", s).is_empty());
        // Nombres propios de funciones y clases (declaracion y expresion).
        assert_eq!(r("function f(){function g(){}return g}", "function f(){function e(){}return e}", s), ["g -> e"]);
        assert_eq!(r("function f(){class C{}return C}", "function f(){class e{}return e}", s), ["C -> e"]);
        assert_eq!(r("function f(){return class C{}}", "function f(){return class e{}}", s), ["C -> e"]);
        // Privados: un metodo, y un campo con una definicion anonima; un campo con un valor no.
        assert_eq!(r("class A{#m(){}g(){return this.#m}}", "class A{#a(){}g(){return this.#a}}", s), ["#m -> #a"]);
        assert_eq!(r("class A{#h=()=>{};g(){return this.#h}}", "class A{#a=()=>{};g(){return this.#a}}", s), ["#h -> #a"]);
        assert!(r("class A{#x=1;g(){return this.#x}}", "class A{#a=1;g(){return this.#a}}", s).is_empty());
        // Modulo: export de un binding con default anonimo.
        assert_eq!(r("let h=()=>{};export{h}", "let e=()=>{};export{e as h}", m), ["h -> e"]);
        // Un global no se renombra (si no, ni siquiera es alfa-equivalente).
        assert!(r("h=()=>{};", "h=()=>{};", s).is_empty());
    }

    /// De punta a punta con oxc y node: el positivo (h1/01 de d2-hallazgos) da Explica; un B
    /// con el mangle roto a mano (la llamada captura otro binding no llamable y el mensaje
    /// tiene la misma forma) da Inconcluso; y un B0 que ya diverge (compress) da NoExplica.
    #[test]
    fn mensaje_nombre_en_node() {
        if !crate::test_util::requiere_node() {
            return;
        }
        let preload = Preload::write_tagged("mnombre").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), std::time::Duration::from_secs(20));
        let src = "function run(cb) {\n  var n = 1;\n  try { cb(); } catch (e) { console.log(e.message); }\n  return n;\n}\nrun();\n";
        let cfg = Preset::CarrilA.build(Minifier::Oxc, SourceKind::Script);
        let Outcome::Ok { original, minified, .. } = prepare(src, &cfg) else { panic!("oxc no minifica") };
        let (a, b) = (r.run(&original), r.run(&minified));
        assert_eq!(a.stdout, "cb is not a function\n");
        assert_ne!(a.stdout, b.stdout, "oxc ya no manglea el parametro: {minified}");
        assert!(mensaje_con_nombre_localizado(&a, &b), "{} | {}", a.stdout, b.stdout);
        let rc = mensaje_nombre_matters(&r, (Minifier::Oxc, Preset::CarrilA, SourceKind::Script), &original, &minified, &a, &b);
        assert_eq!(rc, Recheck::Explica);
        // H1: con keep_names (carril_a), el parametro con default anonimo que oxc 0.153.0
        // renombra cambia el `.name` que el programa imprime con la forma del mensaje:
        // Inconcluso. Sin keep_names (core_defaults) es la asuncion aceptada: Explica.
        let h1 = "function run(p) {\n  const check = (handler = () => {}) => {\n    console.log(handler.name + \" is not a function\");\n  };\n  check();\n  return p;\n}\nrun();\n";
        for (preset, inconcluso) in [(Preset::CarrilA, true), (Preset::CoreDefaults, false)] {
            let cfg = preset.build(Minifier::Oxc, SourceKind::Script);
            let Outcome::Ok { original, minified, .. } = prepare(h1, &cfg) else { panic!("oxc no minifica") };
            let (a, b) = (r.run(&original), r.run(&minified));
            assert_eq!(a.stdout, "handler is not a function\n");
            assert!(mensaje_con_nombre_localizado(&a, &b), "oxc ya no manglea el parametro ({preset:?}): {minified}");
            let rc = mensaje_nombre_matters(&r, (Minifier::Oxc, preset, SourceKind::Script), &original, &minified, &a, &b);
            if inconcluso {
                assert!(matches!(rc, Recheck::Inconcluso(ref n) if n.contains("keep_names") && n.contains("handler -> ")), "{rc:?}");
            } else {
                assert_eq!(rc, Recheck::Explica, "{preset:?}");
            }
        }
        // El mangle roto: `n` (1) en vez del parametro. B da `<otro> is not a function`.
        let m0 = "function run(cb){var n=1;try{cb()}catch(e){console.log(e.message)}return n}run();\n";
        let roto = "function run(t){var r=1;try{r()}catch(e){console.log(e.message)}return r}run();\n";
        let b_roto = r.run(roto);
        assert_eq!(b_roto.stdout, "r is not a function\n");
        let rc = mensaje_nombre_con(&r, (SourceKind::Script, true), roto, m0, &a, &b_roto);
        assert!(matches!(rc, Recheck::Inconcluso(ref n) if n.contains("bug del mangler")), "{rc:?}");
        // B0 con otro mensaje (compress llamo a otra cosa): NoExplica.
        let m0_malo = "function run(cb){var n=1;try{n()}catch(e){console.log(e.message)}return n}run();\n";
        let b_malo = "function run(t){var r=1;try{r()}catch(e){console.log(e.message)}return r}run();\n";
        let rc = mensaje_nombre_con(&r, (SourceKind::Script, true), b_malo, m0_malo, &a, &r.run(b_malo));
        assert_eq!(rc, Recheck::NoExplica);
        // Sin mangle sale lo mismo: el nombre no viene del renombre.
        assert_eq!(mensaje_nombre_con(&r, (SourceKind::Script, true), m0, m0, &a, &r.run(m0)), Recheck::NoExplica);
        // swc no tiene variante sin mangle: inconcluso (su politica no lo pide).
        let rc = mensaje_nombre_matters(&r, (Minifier::Swc, Preset::CarrilA, SourceKind::Script), &original, &minified, &a, &b);
        assert!(matches!(rc, Recheck::Inconcluso(_)), "{rc:?}");
    }
}
