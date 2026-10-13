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
//! es div | known | panic | lint | recursos | codegen (10.1, `--crudo`) y `hash16` el del
//! sintoma (oracle.rs).

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

    /// M23: la corrida termino: el indice nuevo reemplaza al anterior. R2-H05:
    /// y ya no hay una corrida cortada pendiente para esta celda.
    pub fn commit(&self) -> std::io::Result<()> {
        self.file.lock().unwrap_or_else(|e| e.into_inner()).flush()?;
        std::fs::rename(&self.tmp, &self.dest)?;
        let _ = std::fs::remove_file(incompleta_of(&self.dest));
        let _ = std::fs::remove_file(parcial_of(&self.dest));
        Ok(())
    }

    /// M23: corrida interrumpida: el indice a medias no reemplaza al anterior.
    /// R2-H05: queda como `index-<celda>.tsv.incompleta`, la marca que lee
    /// `--cruce` (un SIGKILL deja el `.tmp`, que tambien avisa).
    pub fn abandon(&self) {
        self.dejar_marca(&incompleta_of(&self.dest), &parcial_of(&self.dest));
    }

    /// 10.2b (B-H2 / A-H7 de las revisiones del bloque 10): la corrida termino, pero con escrituras
    /// perdidas o casos sin contar: el indice (al que le pueden faltar filas) no reemplaza al
    /// anterior; queda como `index-<celda>.tsv.parcial`, que `--cruce` avisa como incompleta.
    pub fn abandon_parcial(&self) {
        self.dejar_marca(&parcial_of(&self.dest), &incompleta_of(&self.dest));
    }

    /// El `.tmp` pasa a ser la marca `marca` de esta corrida. 10.2c (N1 de la revision B del bloque
    /// 10, vuelta 2): y se borra la `otra` de una corrida anterior, para que `--cruce` muestre un
    /// solo estado, el de la ultima corrida (antes podia listar `incompleta` e `interrumpida` para la
    /// misma celda). Si el rename falla (A, vuelta 2: antes en silencio), se avisa por stderr y la
    /// marca anterior queda: dice igual que la tabla no es de la ultima corrida.
    fn dejar_marca(&self, marca: &Path, otra: &Path) {
        match std::fs::rename(&self.tmp, marca) {
            Ok(()) => {
                let _ = std::fs::remove_file(otra);
            }
            Err(e) => {
                eprintln!("indice: no pude dejar la marca {} ({e}): --cruce puede no avisar que la ultima corrida de {} no termino", marca.display(), self.cell);
                let _ = std::fs::remove_file(&self.tmp);
            }
        }
    }

    /// Como cerrar el indice al final de la corrida (`oracle::Reporter::finish`).
    pub fn cerrar(&self, cierre: Cierre) -> std::io::Result<()> {
        match cierre {
            Cierre::Completa => self.commit(),
            Cierre::Interrumpida => {
                self.abandon();
                Ok(())
            }
            Cierre::Parcial => {
                self.abandon_parcial();
                Ok(())
            }
        }
    }

    pub fn record(&self, case: &str, dest: &str, kind: &str, hash: u64) {
        let line = format!("{case}\t{}\t{dest}\t{kind}\t{hash:016x}\n", self.cell);
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        // Una sola escritura por linea: si dos procesos apuntan por error a la
        // misma celda, al menos las lineas no se intercalan a la mitad.
        if let Err(e) = f.write_all(line.as_bytes()) {
            eprintln!("indice: no pude escribir: {e}");
            // 10.4 (M1 de rev-b9-A): una linea perdida del indice es un hallazgo que `--cruce`
            // no ve; antes la corrida cerraba `completa` y con 0.
            crate::oracle::write_failed();
        }
    }
}

/// 10.2b: como termino la corrida, para el indice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cierre {
    /// Entera: el indice nuevo reemplaza al anterior.
    Completa,
    /// SIGINT/SIGTERM o el binario cambio (R1-H18): `.incompleta`.
    Interrumpida,
    /// Termino con escrituras perdidas o casos sin contar: `.parcial`.
    Parcial,
}

/// 10.2b (B-H1 de la revision B del bloque 10): `Index::create` fallo, asi que esta corrida no
/// deja indice. La marca `index-<celda>.tsv.parcial` hace que `--cruce` avise que la ultima
/// corrida de la celda no lo dejo (la tabla sigue con el indice de la anterior, si hay). Si
/// tampoco se puede escribir la marca (el directorio no se puede escribir), `Err`.
pub fn marcar_parcial(dir: &Path, cell: &str) -> std::io::Result<()> {
    let dest = file_of(dir, cell);
    std::fs::write(parcial_of(&dest), format!("{HEADER}\n# sin indice: no se pudo crear\n"))?;
    // 10.2c (N1): una sola marca, la de la ultima corrida.
    let _ = std::fs::remove_file(incompleta_of(&dest));
    Ok(())
}

fn file_of(dir: &Path, cell: &str) -> PathBuf {
    dir.join(format!("index-{cell}.tsv"))
}

fn incompleta_of(dest: &Path) -> PathBuf {
    PathBuf::from(format!("{}.incompleta", dest.display()))
}

fn parcial_of(dest: &Path) -> PathBuf {
    PathBuf::from(format!("{}.parcial", dest.display()))
}

/// R2-H05: celdas cuya ultima corrida no termino: `.incompleta` (cortada por
/// SIGINT/SIGTERM) o `.tmp` (en curso, o muerta sin cierre: SIGKILL, abort).
/// 10.2b: `.parcial` (termino con escrituras perdidas o casos sin contar, o sin indice).
fn sin_terminar(dir: &Path) -> Vec<(String, &'static str)> {
    let mut v: Vec<(String, &'static str)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let cell = name.strip_prefix("index-")?;
            if let Some(c) = cell.strip_suffix(".tsv.incompleta") {
                Some((c.to_string(), "interrumpida"))
            } else if let Some(c) = cell.strip_suffix(".tsv.parcial") {
                Some((c.to_string(), "incompleta (escrituras perdidas, casos sin contar o sin indice)"))
            } else {
                cell.strip_suffix(".tsv.tmp").map(|c| (c.to_string(), "en curso o muerta sin cierre"))
            }
        })
        .collect();
    v.sort();
    v
}

struct Row {
    case: String,
    cell: String,
    dest: String,
    kind: String,
}

/// 10.2b (B-H2): tambien las lineas que no tienen los 5 campos (una escritura cortada a la mitad),
/// por archivo: antes se descartaban en silencio.
/// Las celdas, las filas y las lineas ilegibles por archivo.
type Leido = (Vec<String>, Vec<Row>, Vec<(String, usize)>);

fn read_rows(dir: &Path) -> std::io::Result<Leido> {
    let mut cells = BTreeSet::new();
    let mut rows = Vec::new();
    let mut ilegibles = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(cell) = name.strip_prefix("index-").and_then(|n| n.strip_suffix(".tsv")) else {
            continue;
        };
        cells.insert(cell.to_string());
        let text = std::fs::read_to_string(entry.path())?;
        let mut malas = 0;
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            if let [case, cell, dest, kind, _hash] = f[..] {
                rows.push(Row {
                    case: case.into(),
                    cell: cell.into(),
                    dest: dest.into(),
                    kind: kind.into(),
                });
            } else {
                malas += 1;
            }
        }
        if malas > 0 {
            ilegibles.push((name, malas));
        }
    }
    ilegibles.sort();
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
    Ok((cells, rows, ilegibles))
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
        // 7b: el backend lo reconoce backend::Minifier (antes "swc" | "oxc" a mano).
        let backend = if parts.first().is_some_and(|p| crate::backend::Minifier::parse(p).is_some()) { parts.remove(0) } else { "" };
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
        // 7b: la abreviatura la declara el backend (backend::Politica).
        let ab = crate::backend::Minifier::parse(self.backend).map_or(self.backend, |m| m.politica().abreviatura);
        let mut s = if ab.is_empty() { String::new() } else { format!("{ab} ") };
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
        // R2-H11: destinos panic/ y recursos/ en la tabla por caso.
        "Panic" => 'P',
        "Recursos" => 'R',
        other => other.chars().next().unwrap_or('?'),
    }
}

/// La tabla que armaba scripts/matriz.sh: una fila por caso con divergencia
/// (`div/`), con la letra del kind en cada celda donde diverge (M24: el kind
/// puede cambiar entre celdas, p. ej. `delete_*` es Stdout en A y ThrowNew en
/// B). Al final, cuantos hallazgos hay por celda y destino.
pub fn cruce(dir: &Path) -> std::io::Result<String> {
    let (cells, rows, ilegibles) = match read_rows(dir) {
        // R2-H05: sin ninguna corrida completa, el error dice cuales se cortaron.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !sin_terminar(dir).is_empty() => {
            let v: Vec<String> = sin_terminar(dir).into_iter().map(|(c, e)| format!("{c} ({e})")).collect();
            return Err(std::io::Error::new(e.kind(), format!("{e}; corridas sin terminar: {}", v.join(", "))));
        }
        r => r?,
    };
    let shorts: Vec<String> = cells.iter().map(|c| Cell::parse(c).short()).collect();
    let width = shorts.iter().map(String::len).max().unwrap_or(0).max(4) + 1;
    // caso -> kind por celda con div
    let mut by_case: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    let mut counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for r in &rows {
        *counts.entry((r.cell.as_str(), r.dest.as_str())).or_default() += 1;
        // R2-H11: tambien panic/ (P) y recursos/ (R) por caso, no solo el conteo
        // por celda. Si un caso tuviera div y panic en la misma celda, gana div.
        let celda = match r.dest.as_str() {
            "div" => Some(r.kind.as_str()),
            "panic" => Some("Panic"),
            "recursos" => Some("Recursos"),
            _ => None,
        };
        if let Some(k) = celda {
            let e = by_case.entry(r.case.as_str()).or_default().entry(r.cell.as_str()).or_insert(k);
            if r.dest == "div" {
                *e = k;
            }
        }
    }

    let mut out = String::new();
    out.push_str(
        "=== divergencias por caso: letra = kind en la celda (S=Stdout E=ExitStatus T=Timeout N=ThrowNew L=ThrowLost Y=ThrowType), P = panic/, R = recursos/, . = nada\n",
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
    // 10.1: `codegen=` (al final) solo si alguna celda corrio con --crudo y dejo alguno: sin la
    // opcion, la tabla queda como antes.
    let con_codegen = rows.iter().any(|r| r.dest == "codegen");
    for c in &cells {
        let n = |d: &str| counts.get(&(c.as_str(), d)).copied().unwrap_or(0);
        out.push_str(&format!(
            "{c}: div={} known={} panic={} lint={} recursos={}{}\n",
            n("div"),
            n("known"),
            n("panic"),
            n("lint"),
            // b2-int (R1-H08 / R2-H04): timeout o memoria del minificador.
            n("recursos"),
            if con_codegen { format!(" codegen={}", n("codegen")) } else { String::new() }
        ));
    }
    let pendientes = sin_terminar(dir);
    if !pendientes.is_empty() {
        out.push_str("\n=== corridas sin terminar: la tabla usa la ultima corrida completa de la celda; su <out>/<celda>/ puede estar a medias (ver CORRIDA.txt)\n");
        for (c, estado) in pendientes {
            let nota = if cells.contains(&c) { "" } else { " (sin corrida completa: no esta en la tabla)" };
            out.push_str(&format!("{c}: {estado}{nota}\n"));
        }
    }
    if !ilegibles.is_empty() {
        out.push_str("\n=== lineas ilegibles (no tienen los 5 campos: una escritura cortada?): la tabla no las cuenta\n");
        for (archivo, n) in ilegibles {
            out.push_str(&format!("{archivo}: {n}\n"));
        }
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
        // b2-int: recursos/ (timeout o memoria del minificador) tambien se cuenta.
        s.record("fixture__lento", "recursos", "Recursos", 4);
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
        assert!(t.contains("swc+core_defaults+tal_cual+mut3: div=0 known=1 panic=0 lint=0 recursos=1"));
        assert!(t.contains("oxc+core_defaults+tal_cual: div=2 known=0 panic=0 lint=0 recursos=0"));

        // M23: una corrida cortada (abandon) deja el indice anterior intacto.
        let ct = Index::create(&d, "oxc+core_defaults+tal_cual").unwrap();
        ct.record("otro", "div", "Stdout", 9);
        ct.abandon();
        // R2-H05: la tabla es la misma y al final avisa que la ultima no termino.
        let tc = cruce(&d).unwrap();
        assert!(tc.starts_with(t.as_str()), "{tc}");
        assert_eq!(&tc[t.len()..].lines().nth(2), &Some("oxc+core_defaults+tal_cual: interrumpida"), "{tc}");
        // Una corrida en curso (o muerta por SIGKILL) deja el .tmp: tambien avisa.
        let en_curso = Index::create(&d, "swc+carril_a+tal_cual").unwrap();
        assert!(cruce(&d).unwrap().contains("\nswc+carril_a+tal_cual: en curso o muerta sin cierre (sin corrida completa: no esta en la tabla)\n"));
        en_curso.abandon();
        // Correr de nuevo una celda reemplaza solo sus filas.
        let ct = Index::create(&d, "oxc+core_defaults+tal_cual").unwrap();
        ct.commit().unwrap();
        drop(ct);
        let t2 = cruce(&d).unwrap();
        assert!(t2.contains("oxc+core_defaults+tal_cual: div=0"));
        assert!(t2.contains("fixture__12214"));
        // Y como termino, ya no figura entre las sin terminar (la otra si).
        assert!(!t2.contains("oxc+core_defaults+tal_cual: interrumpida"), "{t2}");
        assert!(t2.contains("swc+carril_a+tal_cual: interrumpida"), "{t2}");
        std::fs::remove_dir_all(&d).unwrap();

        // Sin indices: error, no una tabla vacia.
        let vacio = std::env::temp_dir().join(format!("difuzz-test-indice-vacio-{}", std::process::id()));
        std::fs::create_dir_all(&vacio).unwrap();
        assert!(cruce(&vacio).is_err());
        // R2-H05: si solo hay corridas cortadas, el error las nombra.
        Index::create(&vacio, "oxc+carril_a+tal_cual").unwrap().abandon();
        let e = cruce(&vacio).unwrap_err().to_string();
        assert!(e.ends_with("corridas sin terminar: oxc+carril_a+tal_cual (interrumpida)"), "{e}");
        std::fs::remove_dir_all(&vacio).unwrap();
    }

    /// R2-H11: panic/ (P) y recursos/ (R) salen por caso en la tabla, no solo
    /// en el conteo por celda; known/ y lint/ no; div gana si coinciden.
    #[test]
    fn cruce_muestra_panics_por_caso() {
        let d = std::env::temp_dir().join(format!("difuzz-test-indice-panic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let a = Index::create(&d, "oxc+core_defaults+tal_cual").unwrap();
        let b = Index::create(&d, "oxc+core_defaults+tal_cual+carril_b").unwrap();
        a.record("fixture__utf16", "panic", "Panic", 1);
        b.record("fixture__utf16", "panic", "Panic", 1);
        a.record("fixture__lento", "recursos", "Recursos", 2);
        b.record("fixture__mixto", "panic", "Panic", 3);
        b.record("fixture__mixto", "div", "Stdout", 4);
        a.record("fixture__conocida", "known", "Stdout", 5);
        a.record("fixture__lint", "lint", "EscriturasSobreLiterales", 6);
        a.commit().unwrap();
        b.commit().unwrap();
        drop((a, b));
        let t = cruce(&d).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
        let fila = |caso: &str| -> Vec<String> {
            t.lines()
                .find(|l| l.ends_with(caso))
                .unwrap_or_else(|| panic!("falta {caso} en\n{t}"))
                .split_whitespace()
                .map(String::from)
                .collect()
        };
        assert_eq!(fila("fixture__utf16"), ["P", "P", "fixture__utf16"], "{t}");
        assert_eq!(fila("fixture__lento"), ["R", ".", "fixture__lento"], "{t}");
        assert_eq!(fila("fixture__mixto"), [".", "S", "fixture__mixto"], "{t}");
        assert!(!t.lines().any(|l| l.ends_with("fixture__conocida") || l.ends_with("fixture__lint")), "{t}");
        assert!(t.lines().next().unwrap().contains("P = panic/, R = recursos/"), "{t}");
        assert!(t.contains("oxc+core_defaults+tal_cual: div=0 known=1 panic=1 lint=1 recursos=1"), "{t}");
    }

    /// 10.1: `codegen=` al final de la linea por celda solo si alguna celda dejo un codegen/
    /// (`--crudo`); no entra en la tabla por caso (ortogonal, como lint/).
    #[test]
    fn cruce_cuenta_codegen_solo_si_hay() {
        let d = std::env::temp_dir().join(format!("difuzz-test-indice-codegen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let a = Index::create(&d, "oxc+carril_a+tal_cual").unwrap();
        a.record("ne-assign-paren", "div", "Stdout", 1);
        a.commit().unwrap();
        drop(a);
        let sin = cruce(&d).unwrap();
        assert!(sin.contains("oxc+carril_a+tal_cual: div=1 known=0 panic=0 lint=0 recursos=0\n") && !sin.contains("codegen"), "{sin}");
        let b = Index::create(&d, "oxc+core_defaults+tal_cual").unwrap();
        b.record("ne-assign-paren", "codegen", "Stdout", 2);
        b.commit().unwrap();
        drop(b);
        let con = cruce(&d).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
        assert!(con.contains("oxc+carril_a+tal_cual: div=1 known=0 panic=0 lint=0 recursos=0 codegen=0\n"), "{con}");
        assert!(con.contains("oxc+core_defaults+tal_cual: div=0 known=0 panic=0 lint=0 recursos=0 codegen=1\n"), "{con}");
        let fila: Vec<&str> = con.lines().find(|l| l.ends_with("ne-assign-paren")).unwrap().split_whitespace().collect();
        assert_eq!(fila, [".", "S", "ne-assign-paren"], "{con}");
    }

    /// 7b (aceptacion): un backend nuevo (el falso `identidad`) sale en `--cruce`
    /// con la abreviatura que declara su politica (`id`), no partido como si
    /// el backend fuera el preset (R1: `identidad.carril_a+tal_cual`).
    #[test]
    fn cruce_con_backend_nuevo() {
        let d = std::env::temp_dir().join(format!("difuzz-test-indice-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let id = Index::create(&d, "identidad+carril_a+tal_cual").unwrap();
        let o = Index::create(&d, "oxc+carril_a+tal_cual").unwrap();
        id.record("caso_x", "div", "Stdout", 1);
        o.record("caso_x", "div", "Stdout", 1);
        id.commit().unwrap();
        o.commit().unwrap();
        drop((id, o));
        let t = cruce(&d).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
        let cab: Vec<&str> = t.lines().nth(1).unwrap().split_whitespace().collect();
        assert_eq!(cab, ["id", "A.tc", "oxc", "A.tc", "caso"], "{t}");
        assert_eq!(Cell::parse("identidad+carril_a+tal_cual").backend, "identidad");
        assert!(t.contains("identidad+carril_a+tal_cual: div=1"), "{t}");
    }

    /// 10.4 (M1 de rev-b9-A): una linea del indice que no se pudo escribir (disco lleno) cuenta
    /// como escritura perdida (`oracle::write_errors`): la corrida no cierra `completa` ni con 0.
    #[test]
    fn linea_perdida_del_indice_cuenta() {
        if !std::path::Path::new("/dev/full").exists() {
            return;
        }
        let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
        let ix = Index { cell: "oxc+carril_a+tal_cual".into(), file: Mutex::new(full), tmp: "/no/existe.tmp".into(), dest: "/no/existe".into() };
        let antes = crate::oracle::write_errors_hilo();
        ix.record("caso", "div", "Stdout", 1);
        assert_eq!(crate::oracle::write_errors_hilo(), antes + 1);
    }

    /// 10.2b (B-H2 de la revision B del bloque 10): un cierre `Parcial` no reemplaza al indice
    /// vigente y `--cruce` lo avisa; una linea cortada se cuenta como ilegible (antes se
    /// descartaba en silencio); una corrida completa borra la marca.
    #[test]
    fn cierre_parcial_y_lineas_ilegibles() {
        let d = std::env::temp_dir().join(format!("difuzz-test-indice-parcial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = "oxc+carril_a+tal_cual";
        let a = Index::create(&d, c).unwrap();
        a.record("viejo", "div", "Stdout", 1);
        a.cerrar(Cierre::Completa).unwrap();
        drop(a);
        let b = Index::create(&d, c).unwrap();
        b.record("nuevo", "div", "Stdout", 2);
        b.cerrar(Cierre::Parcial).unwrap();
        drop(b);
        let t = cruce(&d).unwrap();
        assert!(t.contains("viejo") && !t.contains("nuevo"), "{t}");
        assert!(t.contains(&format!("\n{c}: incompleta (escrituras perdidas, casos sin contar o sin indice)\n")), "{t}");
        // Una linea cortada a la mitad en el indice vigente.
        let mut f = std::fs::OpenOptions::new().append(true).open(d.join(format!("index-{c}.tsv"))).unwrap();
        f.write_all(b"cortad").unwrap();
        drop(f);
        let t = cruce(&d).unwrap();
        assert!(t.contains("\n=== lineas ilegibles") && t.contains(&format!("\nindex-{c}.tsv: 1\n")), "{t}");
        // `marcar_parcial` (el indice no se pudo crear) y despues una corrida completa.
        std::fs::remove_file(d.join(format!("index-{c}.tsv.parcial"))).unwrap();
        marcar_parcial(&d, c).unwrap();
        assert!(cruce(&d).unwrap().contains(&format!("{c}: incompleta")));
        let e = Index::create(&d, c).unwrap();
        e.cerrar(Cierre::Completa).unwrap();
        drop(e);
        assert!(!cruce(&d).unwrap().contains("incompleta"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// 10.2c (N1 de la revision B del bloque 10, vuelta 2): despues de una corrida parcial y otra
    /// interrumpida (o al reves, o sin indice), `--cruce` muestra un solo estado para la celda, el
    /// de la ultima corrida. Antes listaba los dos.
    #[test]
    fn una_sola_marca_la_de_la_ultima_corrida() {
        let d = std::env::temp_dir().join(format!("difuzz-test-indice-marcas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = "oxc+core_defaults+tal_cual+carril_b";
        let correr = |cierre: Cierre| {
            let ix = Index::create(&d, c).unwrap();
            ix.cerrar(cierre).unwrap();
        };
        let estados = || -> Vec<String> {
            let t = cruce(&d).unwrap();
            t.lines().filter(|l| l.starts_with(&format!("{c}: ")) && !l.contains("div=")).map(String::from).collect()
        };
        let parcial = format!("{c}: incompleta (escrituras perdidas, casos sin contar o sin indice)");
        let interrumpida = format!("{c}: interrumpida");
        correr(Cierre::Completa);
        correr(Cierre::Parcial);
        assert_eq!(estados(), std::slice::from_ref(&parcial));
        correr(Cierre::Interrumpida);
        assert_eq!(estados(), [interrumpida]);
        correr(Cierre::Parcial);
        assert_eq!(estados(), std::slice::from_ref(&parcial));
        correr(Cierre::Interrumpida);
        marcar_parcial(&d, c).unwrap();
        assert_eq!(estados(), [parcial]);
        correr(Cierre::Completa);
        assert!(estados().is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
