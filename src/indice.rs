//! Indice compartido entre celdas de la matriz y su cruce.
//!
//! Cada celda es un proceso distinto. Para no depender de appends atomicos
//! entre procesos, cada celda escribe su propio archivo,
//! `<dir>/index-<celda>.tsv`, y el cruce los lee todos. Una celda que se
//! vuelve a correr reemplaza solo su archivo. Dentro del proceso, los workers
//! escriben bajo un Mutex, una linea por hallazgo:
//!
//! `caso<TAB>celda<TAB>destino<TAB>kind<TAB>hash16`
//!
//! `caso` es el nombre del directorio del hallazgo (`div/<caso>`), `destino`
//! es div | known | panic | lint y `hash16` el del sintoma (oracle.rs).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const HEADER: &str = "# difuzz indice v1: caso\tcelda\tdestino\tkind\thash";

pub struct Index {
    cell: String,
    file: Mutex<std::fs::File>,
    /// M23: se escribe a `index-<celda>.tsv.tmp` y se renombra al terminar
    /// (`commit`): una corrida cortada deja el indice anterior intacto.
    tmp: PathBuf,
    dest: PathBuf,
}

impl Index {
    /// Reemplaza el archivo de esta celda al terminar: lo de una corrida
    /// anterior de la misma celda no se mezcla.
    pub fn create(dir: &Path, cell: &str) -> std::io::Result<Index> {
        std::fs::create_dir_all(dir)?;
        let dest = file_of(dir, cell);
        let tmp = PathBuf::from(format!("{}.tmp", dest.display()));
        let mut file = std::fs::File::create(&tmp)?;
        writeln!(file, "{HEADER}")?;
        Ok(Index {
            cell: cell.to_string(),
            file: Mutex::new(file),
            tmp,
            dest,
        })
    }

    /// M23: la corrida termino: el indice nuevo reemplaza al anterior.
    pub fn commit(&self) -> std::io::Result<()> {
        self.file.lock().unwrap_or_else(|e| e.into_inner()).flush()?;
        std::fs::rename(&self.tmp, &self.dest)
    }

    /// M23: corrida interrumpida: se descarta el indice a medias.
    pub fn abandon(&self) {
        let _ = std::fs::remove_file(&self.tmp);
    }

    pub fn record(&self, case: &str, dest: &str, kind: &str, hash: u64) {
        let line = format!("{case}\t{}\t{dest}\t{kind}\t{hash:016x}\n", self.cell);
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        // Una sola escritura por linea: si dos procesos apuntan por error a la
        // misma celda, al menos las lineas no se intercalan a la mitad.
        if let Err(e) = f.write_all(line.as_bytes()) {
            eprintln!("indice: no pude escribir: {e}");
        }
    }
}

fn file_of(dir: &Path, cell: &str) -> PathBuf {
    dir.join(format!("index-{cell}.tsv"))
}

struct Row {
    case: String,
    cell: String,
    dest: String,
    kind: String,
}

fn read_rows(dir: &Path) -> std::io::Result<(Vec<String>, Vec<Row>)> {
    let mut cells = BTreeSet::new();
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(cell) = name.strip_prefix("index-").and_then(|n| n.strip_suffix(".tsv")) else {
            continue;
        };
        cells.insert(cell.to_string());
        let text = std::fs::read_to_string(entry.path())?;
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            if let [case, cell, dest, kind, _hash] = f[..] {
                rows.push(Row {
                    case: case.into(),
                    cell: cell.into(),
                    dest: dest.into(),
                    kind: kind.into(),
                });
            }
        }
    }
    if cells.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no hay index-*.tsv en {}", dir.display()),
        ));
    }
    // M24: orden por backend, preset (core_defaults primero), forma (en_funcion
    // primero), carril y extras. La celda empieza con el backend desde el
    // backend intercambiable: antes se cortaba en el primer '+' y salia al reves.
    let mut cells: Vec<String> = cells.into_iter().collect();
    cells.sort_by_key(|c| Cell::parse(c).key());
    Ok((cells, rows))
}

/// M24: `backend+preset+forma[+carril_b][+invocar][+mutN]`. Una celda vieja
/// (sin backend) se lee igual, con backend vacio.
struct Cell<'a> {
    backend: &'a str,
    preset: &'a str,
    forma: &'a str,
    carril_b: bool,
    extras: Vec<&'a str>,
}

impl<'a> Cell<'a> {
    fn parse(cell: &'a str) -> Cell<'a> {
        let mut parts: Vec<&str> = cell.split('+').collect();
        let backend = if matches!(parts.first(), Some(&"swc") | Some(&"oxc")) { parts.remove(0) } else { "" };
        let mut it = parts.into_iter();
        let preset = it.next().unwrap_or("");
        let forma = it.next().unwrap_or("");
        let mut carril_b = false;
        let mut extras = Vec::new();
        for p in it {
            if p == "carril_b" {
                carril_b = true;
            } else {
                extras.push(p);
            }
        }
        Cell { backend, preset, forma, carril_b, extras }
    }

    fn key(&self) -> (String, u8, u8, bool, String) {
        let preset = match self.preset {
            "core_defaults" => 0,
            "carril_a" => 1,
            _ => 2,
        };
        let forma = match self.forma {
            "en_funcion" => 0,
            "tal_cual" => 1,
            _ => 2,
        };
        (self.backend.to_string(), preset, forma, self.carril_b, self.extras.join("+"))
    }

    /// `oxc+core_defaults+tal_cual+carril_b+mut3` -> `oxc C.tc.B+m3`.
    fn short(&self) -> String {
        let p = match self.preset {
            "core_defaults" => "C",
            "carril_a" => "A",
            other => other,
        };
        let f = match self.forma {
            "en_funcion" => "fn",
            "tal_cual" => "tc",
            other => other,
        };
        let mut s = if self.backend.is_empty() { String::new() } else { format!("{} ", self.backend) };
        s.push_str(p);
        if !f.is_empty() {
            s.push('.');
            s.push_str(f);
        }
        if self.carril_b {
            s.push_str(".B");
        }
        for e in &self.extras {
            let e = match *e {
                "invocar" => "inv".to_string(),
                m if m.starts_with("mut") => format!("m{}", &m[3..]),
                other => other.to_string(),
            };
            s.push('+');
            s.push_str(&e);
        }
        s
    }
}

/// M24: una letra por kind en la tabla (en vez de X).
fn kind_letter(kind: &str) -> char {
    match kind {
        "Stdout" => 'S',
        "ExitStatus" => 'E',
        "Timeout" => 'T',
        "ThrowNew" => 'N',
        "ThrowLost" => 'L',
        "ThrowType" => 'Y',
        other => other.chars().next().unwrap_or('?'),
    }
}

/// La tabla que armaba scripts/matriz.sh: una fila por caso con divergencia
/// (`div/`), con la letra del kind en cada celda donde diverge (M24: el kind
/// puede cambiar entre celdas, p. ej. `delete_*` es Stdout en A y ThrowNew en
/// B). Al final, cuantos hallazgos hay por celda y destino.
pub fn cruce(dir: &Path) -> std::io::Result<String> {
    let (cells, rows) = read_rows(dir)?;
    let shorts: Vec<String> = cells.iter().map(|c| Cell::parse(c).short()).collect();
    let width = shorts.iter().map(String::len).max().unwrap_or(0).max(4) + 1;
    // caso -> kind por celda con div
    let mut by_case: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    let mut counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for r in &rows {
        *counts.entry((r.cell.as_str(), r.dest.as_str())).or_default() += 1;
        if r.dest == "div" {
            by_case
                .entry(r.case.as_str())
                .or_default()
                .entry(r.cell.as_str())
                .or_insert(r.kind.as_str());
        }
    }

    let mut out = String::new();
    out.push_str(
        "=== divergencias por caso: letra = kind en la celda (S=Stdout E=ExitStatus T=Timeout N=ThrowNew L=ThrowLost Y=ThrowType), . = no diverge\n",
    );
    for sh in &shorts {
        out.push_str(&format!("{sh:<width$}"));
    }
    out.push_str("caso\n");
    let mut lines: Vec<(Vec<bool>, String)> = by_case
        .iter()
        .map(|(case, in_cells)| {
            let mut row = String::new();
            let mut present = Vec::new();
            for c in &cells {
                match in_cells.get(c.as_str()) {
                    Some(k) => {
                        row.push_str(&format!("{:<width$}", kind_letter(k)));
                        present.push(true);
                    }
                    None => {
                        row.push_str(&format!("{:<width$}", "."));
                        present.push(false);
                    }
                }
            }
            (present, format!("{row}{case}"))
        })
        .collect();
    // Primero las filas que divergen en las primeras columnas; despues por caso.
    lines.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, l) in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str("\n=== hallazgos por celda\n");
    for c in &cells {
        let n = |d: &str| counts.get(&(c.as_str(), d)).copied().unwrap_or(0);
        out.push_str(&format!(
            "{c}: div={} known={} panic={} lint={}\n",
            n("div"),
            n("known"),
            n("panic"),
            n("lint")
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M24: celdas reales (con backend), orden, abreviaturas y kind por celda.
    #[test]
    fn cuatro_celdas_reales() {
        let d = std::env::temp_dir().join(format!("difuzz-test-indice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let ab = Index::create(&d, "oxc+carril_a+tal_cual+carril_b").unwrap();
        let ct = Index::create(&d, "oxc+core_defaults+tal_cual").unwrap();
        let cfb = Index::create(&d, "oxc+core_defaults+en_funcion+carril_b+invocar").unwrap();
        let s = Index::create(&d, "swc+core_defaults+tal_cual+mut3").unwrap();
        ct.record("terser__delete_1", "div", "Stdout", 1);
        ab.record("terser__delete_1", "div", "ThrowNew", 1);
        cfb.record("fixture__12214", "div", "ThrowNew", 2);
        ct.record("fixture__12214", "div", "ThrowNew", 2);
        s.record("fixture__x", "known", "Stdout", 3);
        for ix in [&ab, &ct, &cfb, &s] {
            ix.commit().unwrap();
        }
        drop((ab, ct, cfb, s));

        let t = cruce(&d).unwrap();
        let lines: Vec<&str> = t.lines().collect();
        assert!(lines[0].starts_with("=== divergencias por caso: letra = kind"));
        // oxc antes que swc; core_defaults antes que carril_a; en_funcion antes que tal_cual.
        assert_eq!(
            lines[1],
            "oxc C.fn.B+inv oxc C.tc       oxc A.tc.B     swc C.tc+m3    caso"
        );
        assert_eq!(lines[2], "N              N              .              .              fixture__12214");
        assert_eq!(lines[3], ".              S              N              .              terser__delete_1");
        assert!(t.contains("oxc+core_defaults+tal_cual: div=2 known=0 panic=0 lint=0"));
        assert!(t.contains("swc+core_defaults+tal_cual+mut3: div=0 known=1 panic=0 lint=0"));

        // M23: una corrida cortada (abandon) deja el indice anterior intacto.
        let ct = Index::create(&d, "oxc+core_defaults+tal_cual").unwrap();
        ct.record("otro", "div", "Stdout", 9);
        ct.abandon();
        assert_eq!(cruce(&d).unwrap(), t);
        // Correr de nuevo una celda reemplaza solo sus filas.
        let ct = Index::create(&d, "oxc+core_defaults+tal_cual").unwrap();
        ct.commit().unwrap();
        drop(ct);
        let t2 = cruce(&d).unwrap();
        assert!(t2.contains("oxc+core_defaults+tal_cual: div=0"));
        assert!(t2.contains("fixture__12214"));
        std::fs::remove_dir_all(&d).unwrap();

        // Sin indices: error, no una tabla vacia.
        let vacio = std::env::temp_dir().join(format!("difuzz-test-indice-vacio-{}", std::process::id()));
        std::fs::create_dir_all(&vacio).unwrap();
        assert!(cruce(&vacio).is_err());
        std::fs::remove_dir_all(&vacio).unwrap();
    }
}
