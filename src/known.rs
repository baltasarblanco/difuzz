//! Divergencias esperadas: no son hallazgos nuevos. Tres fuentes, en orden.
//!
//! 1. Sensor: flags del preload (runtime) y de checks::static_flags (AST) que
//!    dicen que el programa usa algo que AGENTS.md del minifier declara asumido.
//! 2. Ruta: asunciones conocidas por nombre de fixture (tabla vieja; queda hasta
//!    confirmar que el sensor la cubre).
//! 3. El backlog que el minifier ya conoce (SWC: tests/TODO.txt y
//!    tests/postponed.txt listan fixtures de terser que no cumple). Se lee del corpus.
//!
//! 7a: las tres fuentes son del backend medido (`backend::Politica`); aca solo
//! esta la mecanica.
//!
//! Una divergencia conocida se guarda igual (en `known/`, no en `div/`) y se
//! cuenta aparte: no se pierde, pero no compite por atencion. Su sintoma se
//! persiste: si un caso conocido diverge distinto en otra corrida, vuelve a
//! `div/` (known-sintomas.tsv).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Known {
    backlog: HashSet<String>,
    /// La razon de `known/` de un caso del backlog (`backend::Backlog::razon`).
    backlog_razon: &'static str,
    symptoms: Symptoms,
    /// Tabla de razones del backend medido (`backend::Politica::razones`).
    reasons: &'static [(&'static str, &'static str)],
    /// Asunciones por ruta del backend medido (`backend::Politica::rutas`).
    paths: &'static [(&'static str, &'static str)],
}

impl Known {
    /// 7a/7b: las tablas, las rutas y el backlog son del backend medido
    /// (`Minifier::politica`): un backend sin backlog no lee ninguno. El de
    /// SWC se lee si el corpus es `crates/swc_ecma_minifier/tests`; si las
    /// listas no estan, el set queda vacio y solo aplican las asunciones.
    /// `oracle_id` identifica preload + config: sintomas de otro oraculo no aplican.
    pub fn load(corpus: &Path, out: &Path, oracle_id: u64, minifier: crate::backend::Minifier) -> Known {
        let politica = minifier.politica();
        let mut backlog = HashSet::new();
        if let Some(b) = &politica.backlog {
            for list in b.listas {
                let Ok(text) = std::fs::read_to_string(corpus.join(list)) else {
                    continue;
                };
                for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    backlog.insert(format!("{}{line}", b.prefijo));
                }
            }
        }
        Known {
            backlog,
            backlog_razon: politica.backlog.as_ref().map_or("", |b| b.razon),
            symptoms: Symptoms::load(out, oracle_id),
            reasons: politica.razones,
            paths: politica.rutas,
        }
    }

    pub fn backlog_len(&self) -> usize {
        self.backlog.len()
    }

    pub fn classify(&self, rel: &Path, flags: &[&str]) -> Option<String> {
        if let Some((_, why)) = self.reasons.iter().find(|(f, _)| flags.contains(f)) {
            return Some(format!("sensor, asuncion: {why}"));
        }
        // Una variante (mutate.rs, `caso__mutK`) hereda el backlog de su caso.
        let rel = crate::mutate::rel_base(&rel.to_string_lossy());
        if let Some((_, why)) = self.paths.iter().find(|(p, _)| rel.starts_with(p)) {
            return Some(format!("ruta, {why}"));
        }
        if self.backlog.contains(&rel) {
            return Some(self.backlog_razon.into());
        }
        None
    }

    /// `Err(anterior)` si el caso ya tenia otro sintoma registrado. El anterior
    /// no se pisa: el caso sigue saliendo en `div/` hasta que alguien lo mire y
    /// borre su linea de known-sintomas.tsv.
    pub fn record_symptom(&self, rel: &Path, hash: u64) -> Result<(), u64> {
        self.symptoms.record(&rel.to_string_lossy(), hash)
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.symptoms.save()
    }
}

/// FNV-1a de 64 bits sobre las partes, separadas por un 0. Estable entre
/// versiones de Rust (DefaultHasher no lo garantiza).
pub fn fnv1a(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// `<out>/known-sintomas.tsv`: `caso<TAB>hash` por linea, bajo un encabezado
/// con el id del oraculo. Sobrevive entre corridas (el reporter no lo borra).
struct Symptoms {
    path: PathBuf,
    header: String,
    table: Mutex<HashMap<String, u64>>,
}

impl Symptoms {
    fn load(out: &Path, oracle_id: u64) -> Symptoms {
        let path = out.join("known-sintomas.tsv");
        let header = format!("# difuzz known-sintomas, oraculo {oracle_id:016x}");
        let mut table = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut lines = text.lines();
            if lines.next() == Some(header.as_str()) {
                for line in lines {
                    if let Some((case, h)) = line.split_once('\t') {
                        if let Ok(h) = u64::from_str_radix(h, 16) {
                            table.insert(case.to_string(), h);
                        }
                    }
                }
            }
        }
        Symptoms {
            path,
            header,
            table: Mutex::new(table),
        }
    }

    fn record(&self, case: &str, hash: u64) -> Result<(), u64> {
        let mut t = self.table.lock().unwrap_or_else(|e| e.into_inner());
        match t.get(case) {
            Some(&prev) if prev != hash => Err(prev),
            Some(_) => Ok(()),
            None => {
                t.insert(case.to_string(), hash);
                Ok(())
            }
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let t = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows: Vec<_> = t.iter().collect();
        rows.sort();
        let mut text = format!("{}\n", self.header);
        for (case, h) in rows {
            text.push_str(&format!("{case}\t{h:016x}\n"));
        }
        // M23: temporal + rename (como triage.rs): un corte no deja la tabla a medias.
        let tmp = PathBuf::from(format!("{}.tmp-{}", self.path.display(), std::process::id()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::{Known, Symptoms};
    use std::path::Path;

    #[test]
    fn clasifica_backlog_asunciones_y_el_resto() {
        let dir = std::env::temp_dir().join(format!("difuzz-known-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("TODO.txt"), "dead_code/global_fns/input.js\n\n").unwrap();
        std::fs::write(dir.join("postponed.txt"), "reduce_vars/unsafe_evaluate_object_2/input.js\n").unwrap();
        let k = Known::load(&dir, &dir, 0, crate::backend::Minifier::Swc);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(k.backlog_len(), 2);
        assert!(k.classify(Path::new("terser/compress/dead_code/global_fns/input.js"), &[]).is_some());
        assert!(k.classify(Path::new("terser/compress/dead_code/global_fns__mut2/input.js"), &[]).is_some());
        assert!(k
            .classify(Path::new("terser/compress/reduce_vars/unsafe_evaluate_object_2/input.js"), &[])
            .is_some());
        assert!(k
            .classify(
                Path::new(
                    "terser/compress/issue_597/NaN_and_Infinity_should_not_be_replaced_when_they_are_redefined_evaluate/input.js"
                ),
                &["redefine_global"]
            )
            .is_some_and(|w| w.starts_with("sensor")));
        assert!(k.classify(Path::new("terser/compress/issue_2719/warn/input.js"), &[]).is_some());
        assert!(k.classify(Path::new("fixture/issues/8953/input.js"), &[]).is_none());
        let by_flag = k.classify(Path::new("fixture/issues/8953/input.js"), &["tdz"]);
        assert!(by_flag.is_some_and(|w| w.starts_with("sensor")));
    }

    #[test]
    fn razones_dependen_del_backend() {
        use crate::backend::Minifier;
        let dir = std::env::temp_dir().join(format!("difuzz-known-oxc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("TODO.txt"), "dead_code/global_fns/input.js\n").unwrap();
        let k = Known::load(&dir, &dir, 0, Minifier::Oxc);
        std::fs::remove_dir_all(&dir).unwrap();

        // El backlog y las rutas son de SWC.
        assert_eq!(k.backlog_len(), 0);
        assert!(k.classify(Path::new("terser/compress/dead_code/global_fns/input.js"), &[]).is_none());
        assert!(k.classify(Path::new("terser/compress/issue_2719/warn/input.js"), &[]).is_none());
        // Asuncion de oxc con flag; asunciones de SWC que oxc no declara.
        assert!(k.classify(Path::new("x/input.js"), &["tdz"]).is_some_and(|w| w.contains("oxc")));
        assert!(k.classify(Path::new("x/input.js"), &["proxy"]).is_none());
        assert!(k.classify(Path::new("x/input.js"), &["redefine_global"]).is_none());
        // M13: asunciones de oxc que SWC no documenta.
        for f in ["with", "global_conocido", "extends"] {
            assert!(k.classify(Path::new("x/input.js"), &[f]).is_some_and(|w| w.contains("oxc")), "{f}");
        }
    }

    /// 7b (aceptacion): un backend nuevo (el falso `identidad`) no hereda el
    /// backlog de SWC (431 casos en el corpus real) ni sus asunciones por ruta
    /// ni su tabla de razones: clasifica solo con lo que declara (nada).
    #[test]
    fn backend_nuevo_no_hereda_el_backlog_de_swc() {
        use crate::backend::Minifier;
        let dir = std::env::temp_dir().join(format!("difuzz-known-id-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("TODO.txt"), "dead_code/global_fns/input.js\n").unwrap();
        std::fs::write(dir.join("postponed.txt"), "reduce_vars/unsafe_evaluate_object_2/input.js\n").unwrap();
        let swc = Known::load(&dir, &dir, 0, Minifier::Swc);
        let id = Known::load(&dir, &dir, 0, Minifier::Identidad);
        std::fs::remove_dir_all(&dir).unwrap();

        // Control: con SWC el mismo corpus si tiene backlog.
        assert_eq!(swc.backlog_len(), 2);
        assert_eq!(id.backlog_len(), 0);
        assert!(id.classify(Path::new("terser/compress/dead_code/global_fns/input.js"), &[]).is_none());
        assert!(id.classify(Path::new("terser/compress/issue_2719/warn/input.js"), &[]).is_none());
        for f in ["tdz", "proxy", "redefine_global", "fn_tostring", "with", "profundidad"] {
            assert!(id.classify(Path::new("x/input.js"), &[f]).is_none(), "{f}");
        }
    }

    #[test]
    fn sintomas_persisten_y_detectan_cambios() {
        let dir = std::env::temp_dir().join(format!("difuzz-sintomas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = Symptoms::load(&dir, 7);
        assert_eq!(s.record("a", 1), Ok(()));
        assert_eq!(s.record("a", 1), Ok(()));
        assert_eq!(s.record("a", 2), Err(1));
        s.save().unwrap();
        assert_eq!(Symptoms::load(&dir, 7).record("a", 2), Err(1));
        // Otro oraculo: los sintomas viejos no aplican.
        assert_eq!(Symptoms::load(&dir, 8).record("a", 2), Ok(()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
