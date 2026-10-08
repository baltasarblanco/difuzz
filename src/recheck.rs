//! Re-chequeo de los flags estaticos de oxc `with` y `global_conocido`
//! (pendiente del fuego). Como `extends` (main::extends_matters): un flag
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
//! Los dos solo pueden SACAR un flag (known -> div), nunca ponerlo.
//!
//! b1-b (R1-H01/H02, R2-H02): un re-chequeo solo deja el flag (known/) si es
//! concluyente (`veredicto`). Antes, ante cualquier duda (no parsea, el
//! original no termina, el original modificado toma otro camino) el flag se
//! conservaba y un bug real podia terminar en known/. Ahora la duda va a div/
//! con una nota en meta.txt (`Recheck::Inconcluso`).

use std::panic::{catch_unwind, AssertUnwindSafe};

use swc_common::{sync::Lrc, FileName, Globals, SourceMap, DUMMY_SP, GLOBALS};
use swc_ecma_ast::{BlockStmt, ExprStmt, Program, Stmt};
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

/// b1-b: `a`/`b` son el original y el minificado de verdad; `a2`/`b2`, los dos
/// con la asuncion neutralizada (o satisfecha: stubs, "use strict"). Es
/// concluyente que la asuncion explica la divergencia si:
/// 1. `a2` termina solo y `b2` tambien (un Timeout no es evidencia),
/// 2. `b2` da lo mismo que `a2` (la divergencia desaparece), y
/// 3. el camino se sostiene: o `a2` da lo mismo que `a` (la modificacion no
///    cambia lo que hace el original), o `b` da lo mismo que `a2` (el minificado
///    de verdad ya se comporta como el original bajo la asuncion: es justo lo
///    que el minifier asumio), o la divergencia esta confinada a las lineas que
///    la asuncion cambia (`localizada`).
///
/// DESVIO de la propuesta de los revisores (exigir solo `a2 == a`): con eso los
/// positivos reales de `global_conocido`, `with` y `extends` irian a div/, porque
/// ahi la asuncion cambia el camino a proposito (con `window` definido el
/// original imprime otra cosa). El punto 3 los conserva y sigue mandando a div/
/// los `10-rt-*-miente` de sloppy y global (ahi `b` no es `a2`: el minifier no
/// uso la asuncion, rompio otra rama).
pub fn veredicto(a: &runner::Exec, b: &runner::Exec, a2: &runner::Exec, b2: &runner::Exec) -> Recheck {
    if !a2.completed() {
        return Recheck::Inconcluso(format!("el original modificado no termino ({:?})", a2.status));
    }
    if matches!(b2.status, runner::Status::Timeout | runner::Status::Failed(_)) {
        return Recheck::Inconcluso(format!("el minificado modificado no termino ({:?})", b2.status));
    }
    if !matches!(oracle::compare(a2, b2), oracle::Verdict::Same) {
        return Recheck::NoExplica;
    }
    if mismo_camino(a, a2) || matches!(oracle::compare(a2, b), oracle::Verdict::Same) || localizada(a, b, a2) {
        return Recheck::Explica;
    }
    Recheck::Inconcluso(
        "con la asuncion neutralizada el original toma otro camino (A' != A) y el minificado no se comporta como A'".into(),
    )
}

/// b1-b: la divergencia esta confinada a lo que la asuncion cambia: B termina
/// solo, A, B y A' tienen la misma cantidad de lineas y cada linea en que B
/// difiere de A es una en que A' tambien difiere de A. Es el caso de `with`
/// (corpus completo, issue_1105/assorted_Infinity_NaN_undefined_in_with_scope:
/// oxc reescribe `undefined` y `0/0` suponiendo que no hay scope; dentro del
/// `with` eso da otros valores, ni los de A ni los de A'). Un bug en una linea
/// que la asuncion no toca, o una linea de mas o de menos, no pasa.
/// Tambien lo usa M16 (main::depth_recheck), con A' = A con mas pila.
pub fn localizada(a: &runner::Exec, b: &runner::Exec, a2: &runner::Exec) -> bool {
    let lines = |e: &runner::Exec| oracle::normalize_names(&e.stdout).lines().map(String::from).collect::<Vec<_>>();
    let (la, lb, l2) = (lines(a), lines(b), lines(a2));
    b.completed()
        && la.len() == lb.len()
        && la.len() == l2.len()
        && la.iter().zip(&lb).zip(&l2).all(|((x, y), z)| x == y || x != z)
}

/// b1-b: A y A' recorren el mismo camino: dicen lo mismo, o imprimen lo mismo y
/// los dos lanzan en el mismo punto con otro tipo. Lo segundo es el caso tipico
/// de sloppy contra estricto (funcion en bloque de Anexo B: `g()` da TypeError en
/// sloppy y ReferenceError en estricto; corpus completo, dead_code_2_should_warn).
fn mismo_camino(a: &runner::Exec, a2: &runner::Exec) -> bool {
    let sin_tipo = |e: &runner::Exec| {
        let n = oracle::normalize_names(&e.stdout);
        let pre = format!("{}throw ", runner::DZ);
        let mut lines: Vec<&str> = n.lines().collect();
        if let Some(i) = lines.iter().rposition(|l| l.starts_with(&pre)) {
            lines[i] = "throw";
        }
        lines.join("\n")
    };
    crate::same_exec(a, a2) || (a.threw() && a2.threw() && sin_tipo(a) == sin_tipo(a2))
}

/// b1-b: `Explica` solo si la divergencia desaparece con los globals definidos
/// y el camino se sostiene (`veredicto`).
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
    veredicto(a, b, &a2, &b2)
}

/// b1-b: `Explica` solo si la divergencia desaparece sin el scope del `with` y
/// el camino se sostiene (`veredicto`).
pub fn with_matters(
    r: &runner::Runner,
    recheck: (minify::Preset, SourceKind),
    original: &str,
    a: &runner::Exec,
    b: &runner::Exec,
) -> Recheck {
    let (preset, kind) = recheck;
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
    let cfg = preset.build(backend::Minifier::Oxc, kind);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&neutral, &cfg) else {
        return Recheck::Inconcluso("no pude minificar la variante sin `with`".into());
    };
    let a2 = r.run(&o2);
    if !a2.completed() {
        return Recheck::Inconcluso(format!("la variante sin `with` no termino ({:?})", a2.status));
    }
    let b2 = r.run(&m2);
    veredicto(a, b, &a2, &b2)
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
    use super::{globals_matter, neutralize_with, veredicto, with_matters, Recheck};
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

    /// b1-b: solo es concluyente si A' termina, B' == A' y el camino se sostiene.
    #[test]
    fn veredicto_concluyente() {
        let ok = |s: &str| ex(Status::Exited(0), s);
        let th = |s: &str| ex(Status::Exited(THROW_EXIT), s);
        // Mismo camino (A' == A) y la divergencia desaparece.
        assert_eq!(veredicto(&ok("1\n"), &ok("2\n"), &ok("1\n"), &ok("1\n")), Recheck::Explica);
        // Otro camino, pero B ya se comporta como A' (03-global-positivo).
        assert_eq!(veredicto(&ok("no hay\n"), &ok("hay\n"), &ok("hay\n"), &ok("hay\n")), Recheck::Explica);
        // Mismo punto, otro tipo de throw (sloppy: TypeError, estricto: ReferenceError).
        let (a, a2) = (th("!dz throw TypeError\n"), th("!dz throw ReferenceError\n"));
        assert_eq!(veredicto(&a, &th("!dz throw Error\n"), &a2, &th("!dz throw ReferenceError\n")), Recheck::Explica);
        // `with` (issue_1105): B no es ni A ni A', pero solo difiere en las lineas
        // que el scope cambia.
        assert_eq!(veredicto(&ok("u\n3\n"), &ok("u\n4\n"), &ok("u\nu\n"), &ok("u\nu\n")), Recheck::Explica);
        // ... y si B ademas difiere en una linea que la asuncion no toca, no.
        let r = veredicto(&ok("u\n3\nx\n"), &ok("u\n4\ny\n"), &ok("u\nu\nx\n"), &ok("u\nu\nx\n"));
        assert!(matches!(r, Recheck::Inconcluso(_)), "{r:?}");
        // La divergencia sigue con la asuncion neutralizada.
        assert_eq!(veredicto(&ok("1\n"), &ok("2\n"), &ok("1\n"), &ok("2\n")), Recheck::NoExplica);
        // 10-rt-*-miente: A' toma otra rama y B no es A': un bug real no se esconde.
        let r = veredicto(&ok("rest\nfin\n"), &ok("fin\n"), &ok("navegador\nfin\n"), &ok("navegador\nfin\n"));
        assert!(matches!(r, Recheck::Inconcluso(ref n) if n.contains("otro camino")), "{r:?}");
        // R2-H02: un Timeout en la variante no es evidencia.
        let r = veredicto(&ok("1\n"), &ok("2\n"), &ex(Status::Timeout, ""), &ok("1\n"));
        assert!(matches!(r, Recheck::Inconcluso(ref n) if n.contains("Timeout")), "{r:?}");
        let r = veredicto(&ok("1\n"), &ok("2\n"), &ok("1\n"), &ex(Status::Timeout, ""));
        assert!(matches!(r, Recheck::Inconcluso(_)), "{r:?}");
    }

    #[test]
    fn neutraliza_with() {
        let out = neutralize_with("var o = {a: 1};\nwith (o) { console.log(a); }\n", SourceKind::Script).unwrap();
        assert!(out.contains("{\n    o;\n    {\n        console.log(a);\n    }\n}"), "{out}");
        assert!(neutralize_with("console.log(1);\n", SourceKind::Script).is_none());
    }

    /// Con node: el re-chequeo saca el flag solo si la divergencia sigue.
    #[test]
    fn rechequeos_en_node() {
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("sin node en PATH: test omitido");
            return;
        }
        let preload = Preload::write_tagged("rechk").unwrap();
        let r = Runner::new(&preload, 9985, std::time::Duration::from_secs(20));
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
        assert!(matches!(g, Recheck::Inconcluso(ref n) if n.contains("otro camino")), "{g:?}");
        // Los globals definidos son stubs: se pueden leer y llamar.
        let e = r.run_with_globals("console.log(typeof window, String(window.location.href), typeof document);\n", "window");
        assert_eq!(e.stdout, "function [stub window.location.href] undefined\n");
        // `with` que no explica nada: sin el scope, el programa diverge igual
        // (no aplica: el minificado se re-hace con oxc, que no diverge aca).
        let w = "var o = { x: 1 };\nwith (o) { console.log(typeof o); }\n";
        let aw = r.run(w);
        assert_eq!(with_matters(&r, (Preset::CoreDefaults, SourceKind::Script), w, &aw, &aw), Recheck::Explica);
        // O4 (oxc #24331: `delete (0/0)` -> `delete NaN`) con un `with` al lado que
        // no tiene nada que ver: sin el scope la divergencia sigue, el flag se va.
        let o4 = "var o = {};\nwith (o) { console.log(1); }\nconsole.log(delete (0 / 0));\n";
        let a4 = r.run(o4);
        assert_eq!(with_matters(&r, (Preset::CoreDefaults, SourceKind::Script), o4, &a4, &a4), Recheck::NoExplica);
        // 02-with-positivo (c4): sin el scope el original devuelve otra cosa, y el
        // minificado ya se comporta asi: la asuncion explica la divergencia.
        let p = "function f() { var a = 1; with ({ a: 2 }) { return a; } }\nconsole.log(f());\n";
        let (ap, bp) = (r.run(p), r.run("console.log(1);\n"));
        assert_eq!(with_matters(&r, (Preset::CoreDefaults, SourceKind::Script), p, &ap, &bp), Recheck::Explica);
    }
}
