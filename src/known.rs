//! Divergencias esperadas: no son hallazgos nuevos. Dos fuentes.
//!
//! 1. Asunciones documentadas del minifier (crates/swc_ecma_minifier/AGENTS.md,
//!    "Minifier Semantic Assumptions"): un programa que las viola diverge por
//!    diseño.
//! 2. El backlog que SWC ya conoce: tests/TODO.txt y tests/postponed.txt listan
//!    fixtures de terser que el minifier sabe que no cumple. Se leen del corpus.
//!
//! Una divergencia conocida se guarda igual (en `known/`, no en `div/`) y se
//! cuenta aparte: no se pierde, pero no compite por atencion.

use std::collections::HashSet;
use std::path::Path;

/// (prefijo de ruta relativa al corpus, asuncion que el caso viola)
const ASSUMPTIONS: &[(&str, &str)] = &[
    (
        "terser/compress/issue_597/NaN_and_Infinity_should_not_be_replaced_when_they_are_redefined",
        "asuncion: undefined/NaN/Infinity no se redefinen",
    ),
    (
        "terser/compress/issue_2719/",
        "asuncion: no se usa Function.prototype.caller",
    ),
];

pub struct Known {
    backlog: HashSet<String>,
}

impl Known {
    /// Lee los backlogs si el corpus es `crates/swc_ecma_minifier/tests`. Si no
    /// estan, el set queda vacio y solo aplican las asunciones.
    pub fn load(corpus: &Path) -> Known {
        let mut backlog = HashSet::new();
        for list in ["TODO.txt", "postponed.txt"] {
            let Ok(text) = std::fs::read_to_string(corpus.join(list)) else {
                continue;
            };
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                backlog.insert(format!("terser/compress/{line}"));
            }
        }
        Known { backlog }
    }

    pub fn backlog_len(&self) -> usize {
        self.backlog.len()
    }

    pub fn classify(&self, rel: &Path) -> Option<&'static str> {
        let rel = rel.to_string_lossy();
        if let Some((_, why)) = ASSUMPTIONS.iter().find(|(p, _)| rel.starts_with(p)) {
            return Some(why);
        }
        if self.backlog.contains(rel.as_ref()) {
            return Some("backlog de SWC (tests/TODO.txt o tests/postponed.txt)");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Known;
    use std::path::Path;

    #[test]
    fn clasifica_backlog_asunciones_y_el_resto() {
        let dir = std::env::temp_dir().join(format!("difuzz-known-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("TODO.txt"), "dead_code/global_fns/input.js\n\n").unwrap();
        std::fs::write(dir.join("postponed.txt"), "reduce_vars/unsafe_evaluate_object_2/input.js\n").unwrap();
        let k = Known::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(k.backlog_len(), 2);
        assert!(k.classify(Path::new("terser/compress/dead_code/global_fns/input.js")).is_some());
        assert!(k.classify(Path::new("terser/compress/reduce_vars/unsafe_evaluate_object_2/input.js")).is_some());
        assert!(k
            .classify(Path::new(
                "terser/compress/issue_597/NaN_and_Infinity_should_not_be_replaced_when_they_are_redefined_evaluate/input.js"
            ))
            .is_some());
        assert!(k.classify(Path::new("fixture/issues/8953/input.js")).is_none());
    }
}
