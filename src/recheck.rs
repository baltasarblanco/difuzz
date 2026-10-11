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
pub fn comparar_caminos(b: &runner::Exec, con: &runner::Exec, b2: Option<&runner::Exec>, neutro: &runner::Exec) -> Camino {
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
pub fn camino_entorno(
    r: &runner::Runner,
    minified: &str,
    b: &runner::Exec,
    b2: &runner::Exec,
    (args, env): (&[&str], &[(&str, &str)]),
) -> Camino {
    let con = r.run_con_cobertura(minified, &[], &[]);
    let neutro = r.run_con_cobertura(minified, args, env);
    comparar_caminos(b, &con, Some(b2), &neutro)
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
    let con = r.run_con_cobertura(&texto, &[], &[("DIFUZZ_NEUTRO", "0")]);
    let neutro = r.run_con_cobertura(&texto, &[], &[("DIFUZZ_NEUTRO", "1")]);
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
pub fn camino_sensor_i3(sin: &runner::Runner, con: &runner::Runner, minified: &str, b: &runner::Exec, clases: Option<&[&str]>) -> Camino {
    let c = con.run_con_cobertura(minified, &[], &[("DIFUZZ_MAX_PAUSAS", runner::MAX_PAUSAS_MAYOR)]);
    if let Some(d) = c.sensor_degradado() {
        return Camino::NoSe(format!(
            "A y B se compararon sin el sensor (I3) y B con el sensor (tope {}) tampoco corrio entero ({d})",
            runner::MAX_PAUSAS_MAYOR
        ));
    }
    if !matches!(oracle::compare(b, &c), oracle::Verdict::Same) {
        return Camino::NoSe(format!("con el sensor B no da lo mismo que sin el ({:?})", c.status));
    }
    let s = sin.run_con_cobertura(minified, &[], &[]);
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
pub fn camino_de_b(r: &runner::Runner, i3: Option<(&runner::Runner, &str)>, b: &runner::Exec, clases: Option<&[&str]>) -> Camino {
    match i3 {
        Some((con, minified)) if !r.con_sensor() => camino_sensor_i3(r, con, minified, b, clases),
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
    let con = r.run_con_cobertura(&format!("{PREFIJO_SLOPPY}{base}"), &[], &[]);
    let neutro = r.run_con_cobertura(&format!("{PREFIJO_ESTRICTO}{base}"), &[], &[]);
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
    let lines = |e: &runner::Exec| oracle::normalize_names(&e.stdout).lines().map(String::from).collect::<Vec<_>>();
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
    veredicto_neutralizado(a, b, &a2, &b2, None, || camino_entorno(r, minified, b, &b2, (&[], &[("DIFUZZ_GLOBALS", g.as_str())])))
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
    veredicto_neutralizado(a, b, &a2, &b2, Some(salada), || camino_entorno(r, minified, b, &b2, (&[], &env)))
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
    veredicto_neutralizado(a, b, &a2, &b2, None, || camino_entorno(r, minified, b, &b2, (&[], &env)))
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
/// `i3` (integracion 7.1 + 7.4): el runner con el sensor y el minificado si A y B se
/// compararon sin el sensor (`camino_de_b`).
pub fn violaciones_matter(
    r: &runner::Runner,
    recheck: (backend::Minifier, minify::Preset, SourceKind),
    original: &str,
    a: &runner::Exec,
    b: &runner::Exec,
    i3: Option<(&runner::Runner, &str)>,
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
    veredicto_neutralizado(a, b, &a2, &b2, None, || camino_de_b(r, i3, b, None))
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

/// b7-1: `callee(arg)` con `callee` un identificador libre.
fn llamar(callee: &str, arg: Expr) -> Box<Expr> {
    use swc_ecma_ast::{CallExpr, Callee, ExprOrSpread, Ident};
    Box::new(Expr::Call(CallExpr {
        span: DUMMY_SP,
        callee: Callee::Expr(Box::new(Expr::Ident(Ident::new_no_ctxt(callee.into(), DUMMY_SP)))),
        args: vec![ExprOrSpread { spread: None, expr: Box::new(arg) }],
        ..Default::default()
    }))
}

/// b7-1: parsea, aplica `v` y emite. `Some(None)`: no hay nada que cambiar. `None`: no se
/// pudo (no parsea, no emite o el fuente ya nombra los interruptores): eso no es "nada que
/// cambiar", es no saber.
fn instrumentar<V: VisitMut>(src: &str, kind: SourceKind, mut v: V, cambio: impl Fn(&V) -> bool) -> Option<Option<String>> {
    if src.contains("__difuzzNeutro") {
        return None;
    }
    catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            let cm: Lrc<SourceMap> = Default::default();
            let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
            let mut program = backend::swc::parse_program(&fm, kind)?;
            program.visit_mut_with(&mut v);
            if !cambio(&v) {
                return Some(None);
            }
            emit(cm, &program.apply(fixer(None))).map(Some)
        })
    }))
    .ok()
    .flatten()
}

/// b7-1: el minificado con cada `with (o) S` como `with (__difuzzNeutroWith(o)) S`. Con
/// DIFUZZ_NEUTRO=0 el interruptor devuelve `o` (B tal cual); con 1, un objeto sin
/// propiedades: cada nombre de `S` resuelve afuera, como en `{ o; S; }` (`neutralize_with`).
/// `Some(None)` si no hay `with`; `None` si no se pudo (`instrumentar`).
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
    instrumentar(src, kind, N(false), |n| n.0)
}

/// b7-1: el minificado con cada definicion de helper de coercion que renombra
/// `neutralize_coercion` (las mismas claves) como clave computada
/// `[__difuzzNeutroClave("valueOf")]` (o `[__difuzzNeutroClave(Symbol.toPrimitive)]`).
/// Con DIFUZZ_NEUTRO=0 el interruptor devuelve la clave (B tal cual: un metodo con clave
/// computada se llama igual); con 1, `__dz_<nombre>`, como el renombre del original.
/// `Some(None)` si no hay nada que cambiar; `None` si no se pudo (`instrumentar`).
pub fn interruptores_coercion(src: &str, kind: SourceKind) -> Option<Option<String>> {
    use swc_ecma_ast::{AssignExpr, AssignTarget, ClassMember, ComputedPropName, MemberProp, Prop, PropName, SimpleAssignTarget, Str};
    fn clave_de(k: &PropName) -> Option<Expr> {
        match k {
            PropName::Ident(i) if crate::checks::is_coercion_name(i.sym.as_str()) => {
                Some(Expr::Lit(Lit::Str(Str { span: DUMMY_SP, value: i.sym.as_str().into(), raw: None })))
            }
            PropName::Computed(c) if crate::checks::is_symbol_to_primitive(&c.expr) => Some((*c.expr).clone()),
            _ => None,
        }
    }
    fn clave(k: &mut PropName) -> bool {
        let Some(e) = clave_de(k) else {
            return false;
        };
        *k = PropName::Computed(ComputedPropName { span: DUMMY_SP, expr: llamar(INTERRUPTOR_CLAVE, e) });
        true
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
                let e = match &m.prop {
                    MemberProp::Ident(i) if crate::checks::is_coercion_name(i.sym.as_str()) => {
                        Some(Expr::Lit(Lit::Str(Str { span: DUMMY_SP, value: i.sym.as_str().into(), raw: None })))
                    }
                    MemberProp::Computed(c) if crate::checks::is_symbol_to_primitive(&c.expr) => Some((*c.expr).clone()),
                    _ => None,
                };
                if let Some(e) = e {
                    m.prop = MemberProp::Computed(ComputedPropName { span: DUMMY_SP, expr: llamar(INTERRUPTOR_CLAVE, e) });
                    self.0 = true;
                }
            }
        }
    }
    instrumentar(src, kind, N(false), |n| n.0)
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
        let src = "var o = { valueOf() { return 1; }, toString: f, [Symbol.toPrimitive]: g, x: 1 };\nclass C { toString() {} }\nC.prototype.valueOf = h;\no[Symbol.toPrimitive] = k;\nvar { toString: t } = o;\nconsole.log(o.valueOf());\n";
        let out = interruptores_coercion(src, k).unwrap().unwrap();
        for n in [
            "[__difuzzNeutroClave(\"valueOf\")] ()",
            "[__difuzzNeutroClave(\"toString\")]: f",
            "[__difuzzNeutroClave(Symbol.toPrimitive)]: g",
            "class C {\n    [__difuzzNeutroClave(\"toString\")]()",
            "C.prototype[__difuzzNeutroClave(\"valueOf\")] = h",
            "o[__difuzzNeutroClave(Symbol.toPrimitive)] = k",
        ] {
            assert!(out.contains(n), "{n}: {out}");
        }
        assert!(out.contains("toString: t") && out.contains("o.valueOf()"), "{out}");
        assert_eq!(interruptores_coercion("var { toString } = o;\n", k), Some(None));
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
        let (o, _, a, b) = ab(&format!("let s = \"\";\ntry {{ if (String(x) === \"zz\") s = \"q\"; }} catch (e) {{ {bug} }}\nconsole.log(JSON.stringify(s));\nlet x = 1;\n"));
        let v = violaciones_matter(&r, rc, &o, &a, &b, None);
        assert!(todavia(v.clone()), "r01: {v:?}");
        // Positivos (03-global-positivo y keep_comparisons): B ya no depende de la asuncion.
        let o = "try { window; console.log(\"hay window\"); } catch (e) { console.log(\"no hay\"); }\n";
        let m = "console.log(`hay window`);\n";
        assert_eq!(globals_matter(&r, SourceKind::Script, o, m, &r.run(o), &r.run(m)), Recheck::Explica);
        let o = "var o1 = { valueOf: function () { t1(); } };\nvar o2 = { valueOf: function () { t2(); } };\nvar r1 = o1 <= o2;\n";
        let m = "var o1 = { valueOf: function () { t1(); } };\nvar o2 = { valueOf: function () { t2(); } };\n";
        assert_eq!(coercion_matters(&r, rc, o, m, &r.run(o), &r.run(m)), Recheck::Explica);
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
            let rc = violaciones_matter(&r, (Minifier::Oxc, Preset::CoreDefaults, SourceKind::Script), &original, &a, &b, None);
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
        let rc = violaciones_matter(&r, (Minifier::Oxc, Preset::CoreDefaults, SourceKind::Script), "console.log(1);\n", &a, &a, None);
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
