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
//! Los dos solo pueden SACAR un flag (known -> div), nunca ponerlo: ante
//! cualquier duda (no parsea, el original no termina) se conserva, como antes.

use std::panic::{catch_unwind, AssertUnwindSafe};

use swc_common::{sync::Lrc, FileName, Globals, SourceMap, DUMMY_SP, GLOBALS};
use swc_ecma_ast::{BlockStmt, ExprStmt, Program, Stmt};
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};
use swc_ecma_transforms_base::fixer::fixer;
use swc_ecma_visit::{VisitMut, VisitMutWith};

use crate::minify::{self, SourceKind};
use crate::{backend, oracle, runner};

/// `true` si la divergencia desaparece con los globals definidos (o si no se
/// puede saber): el flag se conserva.
pub fn globals_matter(r: &runner::Runner, kind: SourceKind, original: &str, minified: &str) -> bool {
    let names = catch_unwind(AssertUnwindSafe(|| {
        GLOBALS.set(&Globals::new(), || {
            backend::swc::parse_for(original, kind).map(|p| crate::checks::known_globals_used(&p))
        })
    }))
    .ok()
    .flatten()
    .unwrap_or_default();
    if names.is_empty() {
        return true;
    }
    let g = names.join(",");
    let a2 = r.run_with_globals(original, &g);
    if !a2.completed() {
        return true;
    }
    let b2 = r.run_with_globals(minified, &g);
    matches!(oracle::compare(&a2, &b2), oracle::Verdict::Same)
}

/// `true` si `with` puede explicar la divergencia (desaparece sin el scope, o
/// no se puede saber): el flag se conserva.
pub fn with_matters(r: &runner::Runner, recheck: (minify::Preset, SourceKind), original: &str) -> bool {
    let (preset, kind) = recheck;
    let Some(neutral) = neutralize_with(original, kind) else {
        return true;
    };
    let cfg = preset.build(backend::Minifier::Oxc, kind);
    let minify::Outcome::Ok { original: o2, minified: m2, .. } = minify::prepare(&neutral, &cfg) else {
        return true;
    };
    let a2 = r.run(&o2);
    if !a2.completed() {
        return true;
    }
    let b2 = r.run(&m2);
    matches!(oracle::compare(&a2, &b2), oracle::Verdict::Same)
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
    use super::{globals_matter, neutralize_with, with_matters};
    use crate::minify::{Preset, SourceKind};
    use crate::runner::{Preload, Runner};

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
        assert!(globals_matter(&r, SourceKind::Script, o, m));
        // Una divergencia que no tiene que ver con `document`: el flag se va.
        let o = "typeof document; console.log(1);\n";
        let m = "console.log(2);\n";
        assert!(!globals_matter(&r, SourceKind::Script, o, m));
        // Los globals definidos son stubs: se pueden leer y llamar.
        let e = r.run_with_globals("console.log(typeof window, String(window.location.href), typeof document);\n", "window");
        assert_eq!(e.stdout, "function [stub window.location.href] undefined\n");
        // `with` que no explica nada: sin el scope, el programa diverge igual
        // (no aplica: el minificado se re-hace con oxc, que no diverge aca).
        let w = "var o = { x: 1 };\nwith (o) { console.log(typeof o); }\n";
        assert!(with_matters(&r, (Preset::CoreDefaults, SourceKind::Script), w));
        // O4 (oxc #24331: `delete (0/0)` -> `delete NaN`) con un `with` al lado que
        // no tiene nada que ver: sin el scope la divergencia sigue, el flag se va.
        let o4 = "var o = {};\nwith (o) { console.log(1); }\nconsole.log(delete (0 / 0));\n";
        assert!(!with_matters(&r, (Preset::CoreDefaults, SourceKind::Script), o4));
    }
}
