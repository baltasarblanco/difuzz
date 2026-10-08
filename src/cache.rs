//! Cache en disco de la ejecucion A (el original bajo node).
//!
//! A no depende de la config del minifier: en la matriz (4 celdas, 4 procesos)
//! el mismo original se ejecutaba 4 veces. La clave es el texto exacto que se
//! ejecuta mas el id del oraculo (preload, timeout, version de node).
//!
//! Formato: un archivo por entrada, `<dir>/<hash16>.a`, texto con encabezado y
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
//! - `run_salted` (runner.rs:138) y `node_accepts` (:144) no se cachean: solo
//!   corren ante una divergencia con flags de contenido o un SyntaxError de A
//!   (pocos casos), y la clave tendria que distinguirlos.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::known::fnv1a;
use crate::runner::{Exec, Runner, Status};

const MAGIC: &str = "difuzz-cache-a v1";

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
        let dir = dir.and_then(|d| match std::fs::create_dir_all(&d) {
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
    /// hubo que ejecutar.
    pub fn run(&self, runner: &Runner, js: &str, hits: &mut usize) -> Exec {
        if let Some(a) = self.get(js) {
            *hits += 1;
            return a;
        }
        let a = runner.run(js);
        self.put(js, &a);
        a
    }

    fn entry(&self, js: &str) -> Option<PathBuf> {
        let h = fnv1a(&[self.oracle.as_str(), js]);
        self.dir.as_ref().map(|d| d.join(format!("{h:016x}.a")))
    }

    pub fn get(&self, js: &str) -> Option<Exec> {
        let text = std::fs::read_to_string(self.entry(js)?).ok()?;
        decode(&text, &self.oracle, js)
    }

    pub fn put(&self, js: &str, a: &Exec) {
        let Some(path) = self.entry(js) else {
            return;
        };
        if !matches!(a.status, Status::Exited(_))
            || a.stdout.contains(&self.own_paths)
            || a.stderr.contains(&self.own_paths)
        {
            return;
        }
        let tmp = path.with_file_name(format!(
            ".tmp-{}-{}",
            std::process::id(),
            self.tmp_seq.fetch_add(1, Ordering::Relaxed)
        ));
        let ok = std::fs::write(&tmp, encode(&self.oracle, js, a)).is_ok()
            && std::fs::rename(&tmp, &path).is_ok();
        if !ok {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn node_version() -> String {
    Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "node?".into())
}

/// ```text
/// difuzz-cache-a v1
/// oraculo 1f0c...
/// status 0
/// elapsed_us 41230
/// flags stack,proxy
/// len 12 0 345
/// <stdout><stderr><js>
/// ```
/// `len`: bytes de stdout, stderr y js, en ese orden, pegados tras el encabezado.
fn encode(oracle: &str, js: &str, a: &Exec) -> String {
    let code = match a.status {
        Status::Exited(c) => c,
        _ => unreachable!("put solo guarda Exited"),
    };
    format!(
        "{MAGIC}\noraculo {oracle}\nstatus {code}\nelapsed_us {}\nflags {}\nlen {} {} {}\n{}{}{js}",
        a.elapsed.as_micros(),
        a.flags.join(","),
        a.stdout.len(),
        a.stderr.len(),
        js.len(),
        a.stdout,
        a.stderr,
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
    let [lo, le, lj] = lens[..] else {
        return None;
    };
    if rest.len() != lo + le + lj {
        return None;
    }
    let stdout = rest.get(..lo)?;
    let stderr = rest.get(lo..lo + le)?;
    if rest.get(lo + le..)? != js {
        return None;
    }
    Some(Exec {
        status: Status::Exited(code),
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        flags,
        elapsed: Duration::from_micros(micros),
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

    #[test]
    fn colision_del_nombre_es_miss() {
        let d = tmp("colision");
        let c = CacheA::with_oracle(Some(d.clone()), "o".into());
        c.put("uno", &exec("1\n"));
        // Simula una colision: la entrada de "dos" contiene la de "uno".
        std::fs::copy(c.entry("uno").unwrap(), c.entry("dos").unwrap()).unwrap();
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
        let left: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .collect();
        assert!(left.is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
