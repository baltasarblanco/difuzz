//! Cache en disco de la ejecucion A (el original bajo node).
//!
//! A no depende de la config del minifier: en la matriz (4 celdas, 4 procesos)
//! el mismo original se ejecutaba 4 veces. La clave es el texto exacto que se
//! ejecuta mas el id del oraculo (preload, timeout, version de node).
//!
//! Formato: un archivo por entrada, `<dir>/<oraculo16>/<hash16>.a` (M28: un
//! subdirectorio por oraculo, para poder borrar uno entero; las entradas sueltas
//! `<dir>/<hash16>.a` de antes ya no se leen), texto con encabezado y
//! secciones medidas en bytes (ver `encode`). El nombre es FNV-1a de 64 bits
//! (`known::fnv1a`), pero la entrada guarda tambien el oraculo y el texto
//! ejecutado completos y `get` los compara: una colision del hash es un miss,
//! nunca un resultado equivocado. Por eso 64 bits alcanzan.
//!
//! Concurrencia: se escribe a un temporal unico (pid + contador) en el mismo
//! directorio y se renombra. `rename` es atomico en el mismo filesystem: un
//! lector ve la entrada entera o no la ve. Dos escritores de la misma clave
//! escriben lo mismo (A es determinista); gana el ultimo. Vale entre hilos
//! (`--jobs N`) y entre procesos (las celdas de la matriz).
//!
//! Que se cachea:
//! - Solo `Status::Exited(_)`. Timeout y senal dependen de la carga de la
//!   maquina (o del OOM killer): cachearlos congelaria un descarte espurio.
//!   `Failed` es error del fuzzer.
//! - Solo si la salida no menciona la ruta del script ni la del preload
//!   (`difuzz-<pid>-`, runner.rs:101 y :126): cambian por proceso y por
//!   worker, y B corre en la ruta del worker actual. Un stack con la ruta
//!   vieja seria una divergencia falsa.
//! - M28: la pasada observada del carril B (el envoltorio de modulos,
//!   `Runner::observing`) se cachea con el tag del runner (`Runner::cache_tag`)
//!   sumado al oraculo: el mismo texto corrido por el envoltorio es otra entrada.
//! - `run_salted` (runner.rs:138) y `node_accepts` (:144) no se cachean: solo
//!   corren ante una divergencia con flags de contenido o un SyntaxError de A
//!   (pocos casos), y la clave tendria que distinguirlos.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::known::fnv1a;
use crate::runner::{Exec, Runner, Status};

/// M10: v2 guarda tambien el canal de control (`Exec::ctl`).
const MAGIC: &str = "difuzz-cache-a v2";

/// M3: tercera linea de una entrada envenenada (ver `CacheA::tombstone`).
/// `decode` no la reconoce como `status`: es un miss.
const TOMBSTONE: &str = "status volatil";

pub struct CacheA {
    /// `None`: cache apagada (`--no-cache`), `run` corre siempre.
    dir: Option<PathBuf>,
    /// Texto completo del id (no solo su hash): se compara al leer.
    oracle: String,
    /// Marca de las rutas de este proceso (`difuzz-<pid>-`).
    own_paths: String,
    tmp_seq: AtomicUsize,
}

impl CacheA {
    /// `oracle_parts`: todo lo que cambia la ejecucion de A ademas del texto
    /// (preload, timeout, carril). La version de node se agrega aca.
    pub fn new(dir: Option<PathBuf>, oracle_parts: &[&str]) -> CacheA {
        let node = node_version();
        let mut parts: Vec<&str> = oracle_parts.to_vec();
        parts.push(&node);
        CacheA::with_oracle(dir, format!("{:016x}", fnv1a(&parts)))
    }

    fn with_oracle(dir: Option<PathBuf>, oracle: String) -> CacheA {
        let dir = dir.and_then(|d| match std::fs::create_dir_all(d.join(&oracle)) {
            Ok(()) => Some(d),
            Err(e) => {
                eprintln!("cache: no pude crear {}: {e}; sigo sin cache", d.display());
                None
            }
        });
        CacheA {
            dir,
            oracle,
            own_paths: format!("difuzz-{}-", std::process::id()),
            tmp_seq: AtomicUsize::new(0),
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// `runner.run(js)`, salvo que ya este en la cache. Suma 1 a `hits` si no
    /// hubo que ejecutar. M3: el `bool` dice si A salio de la cache (hit), para
    /// que una divergencia se confirme corriendo A en fresco.
    /// M28: el tag del runner (pasada observada del carril B) entra en la clave.
    pub fn run(&self, runner: &Runner, js: &str, hits: &mut usize) -> (Exec, bool) {
        let tag = runner.cache_tag();
        if let Some(a) = self.get_tagged(tag, js) {
            *hits += 1;
            return (a, true);
        }
        let a = runner.run(js);
        self.put_tagged(tag, js, &a);
        (a, false)
    }

    /// M28: el oraculo de las entradas con `tag` ("" = la ejecucion cruda).
    fn oracle_for(&self, tag: &str) -> String {
        if tag.is_empty() {
            self.oracle.clone()
        } else {
            format!("{} {tag}", self.oracle)
        }
    }

    /// M3: lapida. La entrada de `js` dio una salida que una ejecucion fresca no
    /// reproduce (depende de ppid, worker, cwd, otra corrida...): desde ahora es
    /// un miss para `get` y `put` no la pisa.
    pub fn tombstone(&self, runner: &Runner, js: &str) {
        self.tombstone_tagged(runner.cache_tag(), js);
    }

    fn tombstone_tagged(&self, tag: &str, js: &str) {
        let Some(path) = self.entry(tag, js) else {
            return;
        };
        let tmp = path.with_file_name(format!(
            ".tmp-{}-{}",
            std::process::id(),
            self.tmp_seq.fetch_add(1, Ordering::Relaxed)
        ));
        let text = format!("{MAGIC}\noraculo {}\n{TOMBSTONE}\n", self.oracle_for(tag));
        let ok = std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &path).is_ok();
        if !ok {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    fn is_tombstone(&self, tag: &str, path: &Path) -> bool {
        let oracle = self.oracle_for(tag);
        std::fs::read_to_string(path).is_ok_and(|t| {
            t.lines().nth(1).and_then(|l| l.strip_prefix("oraculo ")) == Some(oracle.as_str())
                && t.lines().nth(2) == Some(TOMBSTONE)
        })
    }

    fn entry(&self, tag: &str, js: &str) -> Option<PathBuf> {
        let h = fnv1a(&[self.oracle_for(tag).as_str(), js]);
        self.dir.as_ref().map(|d| d.join(&self.oracle).join(format!("{h:016x}.a")))
    }

    #[cfg(test)]
    pub fn get(&self, js: &str) -> Option<Exec> {
        self.get_tagged("", js)
    }

    fn get_tagged(&self, tag: &str, js: &str) -> Option<Exec> {
        let text = std::fs::read_to_string(self.entry(tag, js)?).ok()?;
        decode(&text, &self.oracle_for(tag), js)
    }

    #[cfg(test)]
    pub fn put(&self, js: &str, a: &Exec) {
        self.put_tagged("", js, a)
    }

    fn put_tagged(&self, tag: &str, js: &str, a: &Exec) {
        let Some(path) = self.entry(tag, js) else {
            return;
        };
        if !matches!(a.status, Status::Exited(_))
            || a.stdout.contains(&self.own_paths)
            || a.stderr.contains(&self.own_paths)
            || self.is_tombstone(tag, &path)
        {
            return;
        }
        let tmp = path.with_file_name(format!(
            ".tmp-{}-{}",
            std::process::id(),
            self.tmp_seq.fetch_add(1, Ordering::Relaxed)
        ));
        let ok = std::fs::write(&tmp, encode(&self.oracle_for(tag), js, a)).is_ok()
            && std::fs::rename(&tmp, &path).is_ok();
        if !ok {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

pub fn node_version() -> String {
    Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "node?".into())
}

/// ```text
/// difuzz-cache-a v2
/// oraculo 1f0c...
/// status 0
/// elapsed_us 41230
/// flags stack,proxy
/// len 12 0 9 345
/// <stdout><stderr><ctl><js>
/// ```
/// `len`: bytes de stdout, stderr, ctl y js, en ese orden, pegados tras el encabezado.
fn encode(oracle: &str, js: &str, a: &Exec) -> String {
    let code = match a.status {
        Status::Exited(c) => c,
        _ => unreachable!("put solo guarda Exited"),
    };
    format!(
        "{MAGIC}\noraculo {oracle}\nstatus {code}\nelapsed_us {}\nflags {}\nlen {} {} {} {}\n{}{}{}{js}",
        a.elapsed.as_micros(),
        a.flags.join(","),
        a.stdout.len(),
        a.stderr.len(),
        a.ctl.len(),
        js.len(),
        a.stdout,
        a.stderr,
        a.ctl,
    )
}

fn decode(text: &str, oracle: &str, js: &str) -> Option<Exec> {
    let mut rest = text;
    let mut line = || {
        let (l, r) = rest.split_once('\n')?;
        rest = r;
        Some(l)
    };
    if line()? != MAGIC || line()?.strip_prefix("oraculo ")? != oracle {
        return None;
    }
    let code: i32 = line()?.strip_prefix("status ")?.parse().ok()?;
    let micros: u64 = line()?.strip_prefix("elapsed_us ")?.parse().ok()?;
    let flags: Vec<String> = line()?
        .strip_prefix("flags ")?
        .split(',')
        .filter(|f| !f.is_empty())
        .map(String::from)
        .collect();
    let lens: Vec<usize> = line()?
        .strip_prefix("len ")?
        .split(' ')
        .map(|n| n.parse().ok())
        .collect::<Option<_>>()?;
    let [lo, le, lc, lj] = lens[..] else {
        return None;
    };
    if rest.len() != lo + le + lc + lj {
        return None;
    }
    let stdout = rest.get(..lo)?;
    let stderr = rest.get(lo..lo + le)?;
    let ctl = rest.get(lo + le..lo + le + lc)?;
    if rest.get(lo + le + lc..)? != js {
        return None;
    }
    Some(Exec {
        status: Status::Exited(code),
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        flags,
        elapsed: Duration::from_micros(micros),
        ctl: ctl.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(stdout: &str) -> Exec {
        Exec {
            status: Status::Exited(0),
            stdout: stdout.into(),
            stderr: "aviso\n".into(),
            flags: vec!["stack".into(), "proxy".into()],
            elapsed: Duration::from_micros(1234),
            ctl: "dz\nflags proxy,stack\n".into(),
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("difuzz-test-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn hit_miss_y_oraculo_distinto() {
        let d = tmp("hit");
        let c = CacheA::with_oracle(Some(d.clone()), "o1".into());
        assert!(c.get("console.log(1)").is_none());
        c.put("console.log(1)", &exec("1\n"));
        let a = c.get("console.log(1)").expect("hit");
        assert_eq!(a.status, Status::Exited(0));
        assert_eq!(a.stdout, "1\n");
        assert_eq!(a.stderr, "aviso\n");
        assert_eq!(a.flags, ["stack", "proxy"]);
        assert_eq!(a.elapsed, Duration::from_micros(1234));
        assert_eq!(a.ctl, "dz\nflags proxy,stack\n");
        // Otro texto: miss.
        assert!(c.get("console.log(2)").is_none());
        // Otro oraculo (otro preload, otro timeout): miss.
        let c2 = CacheA::with_oracle(Some(d.clone()), "o2".into());
        assert!(c2.get("console.log(1)").is_none());
        // Apagada: nunca guarda ni lee.
        let off = CacheA::with_oracle(None, "o1".into());
        off.put("x", &exec("x"));
        assert!(off.get("console.log(1)").is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn no_guarda_lo_que_depende_del_proceso_o_de_la_carga() {
        let d = tmp("nocache");
        let c = CacheA::with_oracle(Some(d.clone()), "o".into());
        let mut t = exec("");
        t.status = Status::Timeout;
        c.put("a", &t);
        assert!(c.get("a").is_none());
        let ruta = format!("at /tmp/difuzz-{}-w3.cjs:1:1\n", std::process::id());
        c.put("b", &exec(&ruta));
        assert!(c.get("b").is_none());
        // Lanzo (exit 70): eso si se guarda.
        let mut l = exec("!dz throw TypeError\n");
        l.status = Status::Exited(70);
        c.put("c", &l);
        assert_eq!(c.get("c").map(|e| e.status), Some(Status::Exited(70)));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// M3: una entrada envenenada queda como miss y no se vuelve a escribir.
    #[test]
    fn lapida_es_miss_y_put_no_la_pisa() {
        let d = tmp("lapida");
        let c = CacheA::with_oracle(Some(d.clone()), "o".into());
        c.put("js", &exec("1\n"));
        assert!(c.get("js").is_some());
        c.tombstone_tagged("", "js");
        assert!(c.get("js").is_none());
        c.put("js", &exec("2\n"));
        assert!(c.get("js").is_none());
        // Otro oraculo no ve la lapida como suya: puede escribir.
        let c2 = CacheA::with_oracle(Some(d.clone()), "o2".into());
        c2.put("js", &exec("3\n"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// M28: la pasada observada del carril B se cachea aparte de la cruda (mismo
    /// texto, otro tag) y la segunda vez es hit.
    #[test]
    fn pasada_observada_cacheada_con_su_tag() {
        use crate::minify::SourceKind;
        use crate::runner::Preload;
        if !crate::test_util::requiere_node() {
            return;
        }
        let d = tmp("observar");
        let c = CacheA::with_oracle(Some(d.clone()), "o".into());
        let preload = Preload::write_tagged("m28").unwrap();
        let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20)).with_kind(SourceKind::Module);
        let obs = r.observing();
        let js = "export const a = 1;\n";
        let mut hits = 0;
        let (cruda, hit) = c.run(&r, js, &mut hits);
        assert!(!hit && cruda.ok_silent(), "{}", cruda.stdout);
        let (o1, hit) = c.run(&obs, js, &mut hits);
        assert!(!hit, "la cruda no puede servir de observada");
        assert_eq!(o1.stdout, "export a = 1\n");
        let (o2, hit) = c.run(&obs, js, &mut hits);
        assert!(hit && o2.stdout == o1.stdout);
        assert_eq!(hits, 1);
        // Con --invocar el envoltorio cambia: otra entrada.
        let inv = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20)).with_kind(SourceKind::Module).with_invocar(true);
        assert!(!c.run(&inv.observing(), js, &mut hits).1);
        // La lapida de la observada no toca la cruda.
        c.tombstone(&obs, js);
        assert!(!c.run(&obs, js, &mut hits).1);
        assert!(c.run(&r, js, &mut hits).1);
        // Subdirectorio por oraculo.
        assert!(std::fs::read_dir(d.join("o")).unwrap().count() >= 3);
        drop((r, inv));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn colision_del_nombre_es_miss() {
        let d = tmp("colision");
        let c = CacheA::with_oracle(Some(d.clone()), "o".into());
        c.put("uno", &exec("1\n"));
        // Simula una colision: la entrada de "dos" contiene la de "uno".
        std::fs::copy(c.entry("", "uno").unwrap(), c.entry("", "dos").unwrap()).unwrap();
        assert!(c.get("dos").is_none());
        assert!(c.get("uno").is_some());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn escritura_concurrente_desde_hilos() {
        let d = tmp("hilos");
        let c = CacheA::with_oracle(Some(d.clone()), "o".into());
        let big = "x".repeat(200_000);
        std::thread::scope(|s| {
            for w in 0..8 {
                let (c, big) = (&c, &big);
                s.spawn(move || {
                    for i in 0..50 {
                        // Todos escriben y leen las mismas 10 claves.
                        let js = format!("caso{}", i % 10);
                        c.put(&js, &exec(&format!("{js}{big}")));
                        if let Some(a) = c.get(&js) {
                            assert_eq!(a.stdout, format!("{js}{big}"), "lectura parcial en el hilo {w}");
                        }
                    }
                });
            }
        });
        for i in 0..10 {
            assert!(c.get(&format!("caso{i}")).is_some());
        }
        // No quedan temporales.
        let left: Vec<_> = std::fs::read_dir(d.join("o"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .collect();
        assert!(left.is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
