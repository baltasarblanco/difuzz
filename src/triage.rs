//! Triage persistente del lint (checks::dead_writes).
//!
//! `<out>/lint-triage.tsv`, una fila por (caso, hash del minificado):
//!
//! `caso<TAB>hash16<TAB>veredicto<TAB>nota`
//!
//! veredicto: `equivalente` | `bug` | `pendiente`. Se edita a mano. Un
//! sospechoso cuyo (caso, hash) ya es `equivalente` no vuelve a `lint/`. Si el
//! caso tenia filas pero con otro hash, el minificado cambio: vuelve a `lint/`
//! con nota "sintoma nuevo" y se agrega una fila `pendiente` (las viejas no se
//! pisan, como en known-sintomas.tsv). Un caso nuevo entra como `pendiente`.
//! Sobrevive entre corridas: el reporter borra los directorios, no los `.tsv`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::known::fnv1a;

const HEADER: &str = "# difuzz lint-triage: caso\thash del minificado\tequivalente|bug|pendiente\tnota";

pub enum Lint {
    /// Ya triageado como equivalente con este mismo minificado: no se reporta.
    Equivalente,
    /// Va a `lint/` con esta nota.
    Reportar(String),
}

pub struct Triage {
    path: PathBuf,
    /// (caso, hash) -> (veredicto, nota)
    table: Mutex<BTreeMap<(String, u64), (String, String)>>,
}

impl Triage {
    pub fn load(out: &Path) -> Triage {
        let path = out.join("lint-triage.tsv");
        let mut table = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
                let mut f = line.splitn(4, '\t');
                let (Some(case), Some(h)) = (f.next(), f.next()) else {
                    continue;
                };
                let Ok(h) = u64::from_str_radix(h.trim(), 16) else {
                    eprintln!("lint-triage.tsv: hash invalido, ignoro la linea: {line}");
                    continue;
                };
                let verdict = f.next().unwrap_or("pendiente").trim().to_string();
                let note = f.next().unwrap_or("").to_string();
                table.insert((case.to_string(), h), (verdict, note));
            }
        }
        Triage {
            path,
            table: Mutex::new(table),
        }
    }

    pub fn check(&self, rel: &Path, minified: &str) -> Lint {
        let case = rel.to_string_lossy().into_owned();
        let h = fnv1a(&[minified]);
        let mut t = self.table.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((verdict, note)) = t.get(&(case.clone(), h)) {
            if verdict == "equivalente" {
                return Lint::Equivalente;
            }
            let note = if note.is_empty() { "-" } else { note.as_str() };
            return Lint::Reportar(format!("hash {h:016x}, triage {verdict}: {note}"));
        }
        let prev: Vec<String> = t
            .iter()
            .filter(|((c, _), _)| *c == case)
            .map(|((_, ph), (v, _))| format!("{ph:016x} {v}"))
            .collect();
        t.insert((case, h), ("pendiente".into(), String::new()));
        if prev.is_empty() {
            Lint::Reportar(format!("hash {h:016x}, sin triage (agregado como pendiente)"))
        } else {
            Lint::Reportar(format!("hash {h:016x}, sintoma nuevo: antes {}", prev.join(", ")))
        }
    }

    /// Escribe a un temporal y renombra: un corte a mitad de camino no pierde
    /// el triage hecho a mano.
    pub fn save(&self) -> std::io::Result<()> {
        let t = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let mut text = format!("{HEADER}\n");
        for ((case, h), (verdict, note)) in t.iter() {
            text.push_str(&format!("{case}\t{h:016x}\t{verdict}\t{note}\n"));
        }
        let tmp = self.path.with_extension(format!("tsv.tmp-{}", std::process::id()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalente_se_omite_y_hash_nuevo_vuelve_con_nota() {
        let d = std::env::temp_dir().join(format!("difuzz-test-triage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let caso = Path::new("terser/compress/a/input.js");
        let otro = Path::new("terser/compress/b/input.js");

        // Primera corrida: todo es nuevo y queda pendiente.
        let t = Triage::load(&d);
        assert!(matches!(t.check(caso, "({}).x=1"), Lint::Reportar(n) if n.contains("sin triage")));
        assert!(matches!(t.check(otro, "[][0]=1"), Lint::Reportar(_)));
        t.save().unwrap();

        // Triage a mano: el primero es equivalente.
        let path = d.join("lint-triage.tsv");
        let text = std::fs::read_to_string(&path).unwrap();
        let h = format!("{:016x}", fnv1a(&["({}).x=1"]));
        assert!(text.contains(&format!("terser/compress/a/input.js\t{h}\tpendiente\t\n")));
        let text = text.replace(
            &format!("{h}\tpendiente\t"),
            &format!("{h}\tequivalente\tel objeto no se usa despues"),
        );
        std::fs::write(&path, text).unwrap();

        // Segunda corrida: mismo hash se omite; el pendiente vuelve con su veredicto.
        let t = Triage::load(&d);
        assert!(matches!(t.check(caso, "({}).x=1"), Lint::Equivalente));
        assert!(matches!(t.check(otro, "[][0]=1"), Lint::Reportar(n) if n.contains("pendiente")));
        // El minificado cambio: vuelve con nota y la fila vieja no se pisa.
        match t.check(caso, "({}).x=2") {
            Lint::Reportar(n) => assert!(n.contains("sintoma nuevo") && n.contains(&format!("{h} equivalente"))),
            Lint::Equivalente => panic!("hash distinto no puede omitirse"),
        }
        t.save().unwrap();
        let t = Triage::load(&d);
        assert!(matches!(t.check(caso, "({}).x=1"), Lint::Equivalente));
        assert!(std::fs::read_to_string(&path).unwrap().contains("el objeto no se usa despues"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
