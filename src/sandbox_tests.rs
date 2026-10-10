//! n-sandbox: `--sandbox` y `--sandbox-red` con node de verdad.
//!
//! - El runner: con el sandbox, leer fuera del directorio de trabajo (un senuelo propio,
//!   `/etc/passwd`, una ruta inexistente de `~/.ssh`), escribir afuera, `child_process`,
//!   workers y `process.binding` fallan y la ejecucion queda marcada (`toco_sandbox`); lo de
//!   adentro del directorio de trabajo anda. Sin el sandbox, lo de siempre (y nunca marca).
//! - El caso entero (`process_case`, como e2e_tests): si el original toca el sandbox, el caso
//!   va a descartes con causa `sandbox`, nunca a div/ ni a known/; sin el flag, el mismo
//!   programa (con un "bug" puesto a mano) va a div/.
//! - Las senales (2b): con el sandbox, `process.kill` a otro proceso da EPERM (y marca); a si
//!   mismo, a 0 y a su grupo anda.
//! - La red (`--sandbox-red`, solo Linux con user namespaces): sin salida (ENETUNREACH), el
//!   loopback propio anda, los sockets unix del sistema quedan tapados; con `--sandbox`
//!   solo, un socket unix de afuera se alcanza (limite documentado del modelo de node).
//!
//! Nada lee secretos: `/etc/passwd` y `~/.ssh` solo se tocan CON el sandbox (se verifica el
//! bloqueo, no el contenido), y en `~/.ssh` con un nombre que no existe.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::backend::identidad::Identidad;
use crate::backend::{Backend, Minifier};
use crate::minify::{Config, Outcome, SourceKind};
use crate::runner::{Preload, Runner, Sandbox, Status};

/// Lo de `Identidad` con `de` cambiado por `a` en el minificado (un "bug" a mano).
struct Cambia {
    de: String,
    a: String,
}

impl Backend for Cambia {
    fn prepare(&self, src: &str) -> Outcome {
        match (Identidad { kind: SourceKind::Script }).prepare(src) {
            Outcome::Ok { original, minified, .. } => {
                assert!(minified.contains(&self.de), "Cambia: {:?} no esta en {minified:?}", self.de);
                let minified = minified.replacen(&self.de, &self.a, 1);
                let (static_flags, dead_writes, analisis) =
                    crate::backend::swc::checks_on_text(src, &original, &minified, SourceKind::Script, Minifier::Identidad);
                Outcome::Ok { original, minified, static_flags, dead_writes, analisis }
            }
            o => o,
        }
    }
}

/// Directorio propio del test (fuera del directorio de trabajo del runner) con un senuelo.
fn base(tag: &str) -> (PathBuf, PathBuf) {
    let b = std::env::temp_dir().join(format!("difuzz-test-{}-sandbox-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&b);
    std::fs::create_dir_all(&b).unwrap();
    let senuelo = b.join("senuelo.txt");
    std::fs::write(&senuelo, "senuelo de difuzz: no es un secreto\n").unwrap();
    (b, senuelo)
}

fn js_str(p: &Path) -> String {
    format!("{:?}", p.to_str().unwrap())
}

/// Corre `cuerpo` adentro de un try (imprime `ok` o `err <code>`) y devuelve el stdout,
/// si toco el sandbox y el estado.
fn intento(r: &Runner, cuerpo: &str) -> (String, bool, Status) {
    let js = format!("try {{ {cuerpo}; console.log('ok'); }} catch (e) {{ console.log('err', e.code); }}\n");
    let e = r.run(&js);
    (e.stdout.trim().to_string(), e.toco_sandbox(), e.status)
}

#[test]
fn sandbox_bloquea_y_marca() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let (b, senuelo) = base("runner");
    let preload = Preload::write_tagged("sandbox").unwrap();
    let t = Duration::from_secs(20);
    let con = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(Sandbox::Permisos);
    let sin = Runner::new(&preload, crate::test_util::id_runner(), t);
    let fuera = b.join("escrito-desde-node.txt");
    let ssh = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".into())).join(".ssh/difuzz-senuelo-que-no-existe");
    let fs = "require('fs')";
    let negados = [
        format!("{fs}.readFileSync({})", js_str(&senuelo)),
        format!("{fs}.readFileSync('/etc/passwd')"),
        format!("{fs}.readFileSync({})", js_str(&ssh)),
        format!("{fs}.writeFileSync({}, 'x')", js_str(&fuera)),
        "require('child_process').execSync('true')".into(),
        "new (require('worker_threads').Worker)('1', { eval: true })".into(),
        "process.binding('fs')".into(),
    ];
    for c in &negados {
        let (out, toco, st) = intento(&con, c);
        assert!(out.starts_with("err ERR_ACCESS_DENIED"), "{c}: {out}");
        assert!(toco, "{c}: no marco el sandbox");
        assert_eq!(st, Status::Exited(0), "{c}");
    }
    assert!(!fuera.exists(), "el sandbox dejo escribir fuera del directorio de trabajo");
    // Adentro del directorio de trabajo (el cwd del programa) se lee y se escribe.
    let (out, toco, _) = intento(&con, &format!("{fs}.writeFileSync('a.txt', 'x'); if ({fs}.readFileSync('a.txt', 'utf8') !== 'x') throw 1"));
    assert_eq!((out.as_str(), toco), ("ok", false));
    // Un symlink del directorio de trabajo hacia el senuelo no sirve para salir.
    let (out, toco, _) = intento(&con, &format!("{fs}.symlinkSync({}, 'l'); {fs}.readFileSync('l')", js_str(&senuelo)));
    assert!(out.starts_with("err ERR_ACCESS_DENIED") && toco, "symlink: {out}");
    // Sin atrapar: lanza (exit 70) y tambien marca (el preload lo ve en onThrow).
    let e = con.run(&format!("{fs}.readFileSync({});\n", js_str(&senuelo)));
    assert!(e.threw() && e.toco_sandbox(), "{:?} {}", e.status, e.ctl);
    // Asincronico, atrapado por el programa: lo ve la salida.
    let e = con.run(&format!("{fs}.readFile({}, (err) => console.log(err && err.code));\n", js_str(&senuelo)));
    assert!(e.stdout.contains("ERR_ACCESS_DENIED") && e.toco_sandbox(), "{}", e.stdout);

    // Sin el sandbox, lo de siempre: el senuelo se lee, la ruta de ~/.ssh no existe (ENOENT,
    // no se leyo nada) y nada marca, ni un programa que imprime el texto de la marca.
    let (out, toco, _) = intento(&sin, &format!("{fs}.readFileSync({})", js_str(&senuelo)));
    assert_eq!((out.as_str(), toco), ("ok", false));
    let (out, toco, _) = intento(&sin, &format!("{fs}.readFileSync({})", js_str(&ssh)));
    assert_eq!((out.as_str(), toco), ("err ENOENT", false));
    let e = sin.run("console.log('ERR_ACCESS_DENIED');\n");
    assert!(!e.toco_sandbox());
    // Una linea `sandbox` en el canal (la escribiria un programa que adivina su ruta) no
    // cambia nada sin el sandbox: main pregunta al runner.
    let mut e = sin.run("console.log(1);\n");
    e.ctl.push_str("sandbox\n");
    assert!(e.toco_sandbox() && !sin.toco_sandbox(&e) && con.toco_sandbox(&e));
    drop((con, sin));
    let _ = std::fs::remove_dir_all(&b);
}

/// El sensor de violaciones sigue andando con el sandbox (`--allow-inspector`), y un
/// permiso negado que el programa atrapa en silencio lo ve el sensor.
#[test]
fn sandbox_con_sensor() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let (b, senuelo) = base("sensor");
    let preload = Preload::write_tagged("sandbox-sensor").unwrap();
    let r = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_secs(20)).with_sandbox(Sandbox::Permisos);
    let e = r.run("try { x; } catch (e) { console.log('atrapada'); }\nlet x = 1;\n");
    assert!(e.flags.iter().any(|f| f == "tdz_atrapada"), "{:?} {}", e.flags, e.ctl);
    assert!(!e.toco_sandbox());
    let e = r.run(&format!("try {{ require('fs').readFileSync({}); }} catch {{}}\nconsole.log(1);\n", js_str(&senuelo)));
    assert_eq!(e.stdout.trim(), "1");
    assert!(e.toco_sandbox(), "atrapado en silencio: {}", e.ctl);
    drop(r);
    let _ = std::fs::remove_dir_all(&b);
}

/// Un caso entero: con `--sandbox`, el original que toca un permiso (y su minificado
/// tambien) va a descartes (`sandbox`); sin el flag, el mismo programa con el "bug" va a
/// div/. Y un minificado que toca el sandbox cuando el original no (el bug cambia la ruta)
/// se compara: div/. 2c: y al reves, un original que lo toca cuando el minificado no (el
/// bug hace que B no lea), tambien: div/, con la nota en meta.txt.
#[test]
fn sandbox_destino_del_caso() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let (b, senuelo) = base("caso");
    let (corpus, out) = (b.join("corpus"), b.join("out"));
    let leer = format!("var t = require('fs').readFileSync({}, 'utf8');\nconsole.log(\"bug-no\", t.length);\n", js_str(&senuelo));
    let hijo = "require('child_process').execSync('true');\nconsole.log(\"bug-no\");\n".to_string();
    let escribir = "require('fs').writeFileSync('dentro.txt', 'x');\nconsole.log(41);\n".to_string();
    // revision-n2/sb3/u01-solo-a-toca: el original lee (negado, atrapado e impreso); con el
    // "bug", B no entra a la rama.
    let solo_a = format!(
        "var m = \"bug-no\";\nif (m === \"bug-no\") {{ try {{ require('fs').readFileSync({}); }} catch (e) {{ console.log(\"negado\", e.code); }} }}\nconsole.log(m);\n",
        js_str(&senuelo)
    );
    // (caso, programa, (de, a) del "bug", flags, destino esperado)
    type Caso<'a> = (&'a str, String, (&'a str, String), &'a [&'a str], &'a str);
    let casos: Vec<Caso> = vec![
        ("leer-con", leer.clone(), ("bug-no", "bug-si".into()), &["--sandbox"], "descarte:sandbox"),
        ("leer-sin", leer, ("bug-no", "bug-si".into()), &[], "div"),
        ("hijo-con", hijo.clone(), ("bug-no", "bug-si".into()), &["--sandbox"], "descarte:sandbox"),
        ("hijo-sin", hijo, ("bug-no", "bug-si".into()), &[], "div"),
        // El original escribe adentro; el minificado (el bug) escribe afuera: B-solo.
        ("solo-b", escribir, ("dentro.txt", b.join("fuera.txt").to_str().unwrap().to_string()), &["--sandbox"], "div"),
        ("solo-a", solo_a, ("bug-no", "bug-si".into()), &["--sandbox"], "div"),
    ];
    let preload = Preload::write_tagged("sandbox-caso").unwrap();
    let known = crate::known::Known::load(&corpus, &out, 0, Minifier::Oxc);
    let cache = crate::cache::CacheA::new(None, &[]);
    let triage = crate::triage::Triage::load(&out);
    for (caso, js, (de, a), extra, esperado) in casos {
        std::fs::create_dir_all(corpus.join(caso)).unwrap();
        std::fs::write(corpus.join(caso).join("input.js"), &js).unwrap();
        let mut args: Vec<String> = [corpus.to_str().unwrap(), "--out", out.to_str().unwrap(), "--no-cache", "--jobs", "1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        args.extend(extra.iter().map(|s| s.to_string()));
        let cli = crate::Cli::parse(args).unwrap();
        let celda = out.join(caso);
        let reporter = crate::oracle::Reporter::new(&celda, cli.cell(), crate::runner::PRELOAD);
        let runner = crate::runner_de_celda(&preload, crate::test_util::id_runner(), &cli);
        let cfg = Config::from_backend(Box::new(Cambia { de: de.into(), a }));
        let ctx = crate::Ctx { reporter: &reporter, known: &known, cache: &cache, triage: &triage, medido: None };
        let mut st = crate::Stats::default();
        crate::process_case(&ctx, &runner, &cfg, None, &cli, &mut st, &corpus.join(caso).join("input.js"));
        drop(runner);
        let tsv = std::fs::read_to_string(celda.join("descartes.tsv")).unwrap_or_default();
        let causa = tsv.lines().filter(|l| !l.starts_with('#')).find_map(|l| l.split('\t').nth(1).map(|c| format!("descarte:{c}")));
        let donde = if celda.join("div").join(caso).exists() {
            "div".to_string()
        } else if celda.join("known").join(caso).exists() {
            "known".to_string()
        } else {
            causa.clone().unwrap_or_else(|| "iguales".into())
        };
        assert_eq!(donde, esperado, "{caso}: {} | {tsv}", st.line());
        assert_eq!(st.a_sandbox, usize::from(esperado == "descarte:sandbox"), "{caso}");
        assert_eq!(st.solo_a_sandbox, usize::from(caso == "solo-a"), "{caso}");
        if esperado == "div" {
            assert!(causa.is_none(), "{caso}: div/ y ademas descarte {causa:?}");
            let meta = std::fs::read_to_string(celda.join("div").join(caso).join("meta.txt")).unwrap();
            assert_eq!(meta.contains("solo el original toco el sandbox"), caso == "solo-a", "{caso}: {meta}");
        }
    }
    drop(preload);
    let _ = std::fs::remove_dir_all(&b);
}

/// 2b: con `--sandbox` (y con `--sandbox-red`, si esta maquina lo arma), node no le manda
/// senales a otro proceso: EPERM, y la ejecucion queda marcada. El senuelo (un `sleep`
/// propio) recibe SIGTERM solo si el filtro falla; a este test (el padre de node) y a todo el
/// usuario (`-1`) se les manda la senal 0, que no hace nada aunque llegue. `process.pid` y
/// `process.ppid` los fija el preload (4242 y 4241, M20): no son node ni su padre, y con el
/// sandbox tambien dan EPERM. A si mismo (el pid real, de `process.report`), a 0 y a su
/// grupo (pgid = pid de node) anda. Sin el sandbox, lo de siempre.
#[test]
fn sandbox_senales() {
    if !crate::test_util::requiere_node() {
        return;
    }
    let preload = Preload::write_tagged("sandbox-senales").unwrap();
    let t = Duration::from_secs(20);
    let mut modos = vec![Sandbox::Permisos];
    match crate::runner::probar_sandbox(Sandbox::PermisosYRed) {
        Ok(()) => modos.push(Sandbox::PermisosYRed),
        Err(e) => eprintln!("DIFUZZ_SALTEADO_SIN_USERNS: sandbox_senales con la red: {e}"),
    }
    let mut senuelo = std::process::Command::new("sleep").arg("60").spawn().unwrap();
    let (pid, padre) = (senuelo.id(), std::process::id());
    let yo = "process.report.getReport().header.processId";
    for modo in modos {
        let r = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(modo);
        let afuera = [
            (pid.to_string(), "'SIGTERM'"),
            (padre.to_string(), "0"),
            ("-1".into(), "0"),
            ("process.pid".into(), "0"),
            ("process.ppid".into(), "0"),
        ];
        for (destino, senal) in afuera {
            let (out, toco, st) = intento(&r, &format!("process.kill({destino}, {senal})"));
            assert_eq!((out.as_str(), toco, st), ("err EPERM", true, Status::Exited(0)), "{modo:?} {destino}");
        }
        // Sin atrapar: lanza y marca (el mensaje `kill EPERM` en la linea `exc`).
        let e = r.run(&format!("process.kill({pid}, 'SIGTERM');\n"));
        assert!(e.threw() && e.toco_sandbox(), "{modo:?}: {:?} {}", e.status, e.ctl);
        for destino in [yo.to_string(), "0".into(), format!("-{yo}")] {
            let (out, toco, _) = intento(&r, &format!("process.kill({destino}, 0)"));
            assert_eq!((out.as_str(), toco), ("ok", false), "{modo:?} {destino}");
        }
        // Y la senal llega: un handler propio la recibe (el timer mantiene vivo el loop hasta
        // que llega; sin el, node termina antes tambien sin el sandbox: MEDIDO).
        let e = r.run(&format!("process.on('SIGUSR2', () => console.log('recibida'));\nprocess.kill({yo}, 'SIGUSR2');\nsetTimeout(() => {{}}, 50);\n"));
        assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("recibida", false), "{modo:?}: {}", e.stderr);
        assert!(senuelo.try_wait().unwrap().is_none(), "{modo:?}: el senuelo recibio la senal");
    }
    let sin = Runner::new(&preload, crate::test_util::id_runner(), t);
    let (out, toco, _) = intento(&sin, &format!("process.kill({pid}, 0)"));
    assert_eq!((out.as_str(), toco), ("ok", false));
    let _ = senuelo.kill();
    let _ = senuelo.wait();
}

/// La celda y la cache: `--sandbox` y `--sandbox-red` cambian el nombre de la celda; sin
/// el flag, el de siempre.
#[test]
fn sandbox_en_la_celda() {
    let cli = |extra: &[&str]| crate::Cli::parse(["c"].iter().chain(extra).map(|s| s.to_string()).collect()).unwrap();
    assert_eq!(cli(&[]).cell(), "oxc+carril_a+tal_cual");
    assert_eq!(cli(&["--sandbox"]).cell(), "oxc+carril_a+tal_cual+sandbox");
    assert_eq!(cli(&["--sandbox-red"]).cell(), "oxc+carril_a+tal_cual+sandbox_red");
    assert_eq!(cli(&["--sandbox-red", "--sandbox"]).sandbox, Sandbox::PermisosYRed);
    assert_eq!(cli(&["--stubs", "--sandbox"]).cell(), "oxc+carril_a+tal_cual+stubs+sandbox");
}

/// `--sandbox-red`: solo si esta maquina puede armar los namespaces (si no, se saltea con
/// una linea en stderr; `probar_sandbox` es lo que corta la corrida en ese caso).
#[test]
fn sandbox_red() {
    if !crate::test_util::requiere_node() {
        return;
    }
    if let Err(e) = crate::runner::probar_sandbox(Sandbox::PermisosYRed) {
        eprintln!("DIFUZZ_SALTEADO_SIN_USERNS: sandbox_red: {e}");
        return;
    }
    let (b, _) = base("red");
    // Un socket unix "del sistema" (fuera de /run): uno propio, escuchando.
    let sock = b.join("servicio.sock");
    let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    l.set_nonblocking(true).unwrap();
    let preload = Preload::write_tagged("sandbox-red").unwrap();
    let t = Duration::from_secs(20);
    let red = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(Sandbox::PermisosYRed);
    let solo = Runner::new(&preload, crate::test_util::id_runner(), t).with_sandbox(Sandbox::Permisos);
    let conectar = |destino: &str| {
        format!(
            "const s = require('net').connect({destino});\
             s.on('connect', () => {{ console.log('conecto'); s.destroy(); }});\
             s.on('error', (e) => console.log('err', e.code));\n"
        )
    };
    // Afuera (192.0.2.1 es TEST-NET-1, documentacion): sin ruta en el namespace, no sale nada.
    let e = red.run(&conectar("9, '192.0.2.1'"));
    assert_eq!(e.stdout.trim(), "err ENETUNREACH");
    assert!(e.toco_sandbox());
    // El loopback propio anda: un servidor y un cliente del mismo programa.
    let e = red.run(
        "const net = require('net');\
         const srv = net.createServer((c) => c.end('hola')).listen(0, '127.0.0.1', () => {\
           const c = net.connect(srv.address().port, '127.0.0.1');\
           c.on('data', (d) => { console.log(String(d)); srv.close(); });\
         });\n",
    );
    assert_eq!((e.stdout.trim(), e.toco_sandbox()), ("hola", false), "{}", e.stderr);
    // DNS: sin /run no hay resolvedor; un nombre que no esta en /etc/hosts no resuelve (y
    // no sale nada: no hay red). Marca.
    let e = red.run("require('dns').lookup('difuzz-no-existe.invalid', (err) => console.log('dns', err && err.code));\n");
    assert!(e.stdout.starts_with("dns E") && e.toco_sandbox(), "{}", e.stdout);
    // El socket unix: con --sandbox solo se alcanza (limite de node); con la red, tapado.
    let e = solo.run(&conectar(&js_str(&sock)));
    assert_eq!(e.stdout.trim(), "conecto");
    let e = red.run(&conectar(&js_str(&sock)));
    assert!(e.stdout.trim().starts_with("err "), "{}", e.stdout);
    // /run tapado: el socket de systemd-resolved, si esta en esta maquina, se alcanza con
    // --sandbox solo (MEDIDO: por ahi resuelve DNS) y no con la red. Solo se conecta, no
    // se le manda nada.
    let resolved = Path::new("/run/systemd/resolve/io.systemd.Resolve");
    if resolved.exists() {
        let e = solo.run(&conectar(&js_str(resolved)));
        assert_eq!(e.stdout.trim(), "conecto");
        let e = red.run(&conectar(&js_str(resolved)));
        assert!(e.stdout.trim().starts_with("err "), "{}", e.stdout);
    }
    // El mismo uid adentro, y el timeout mata al node del namespace.
    // SAFETY: getuid no falla.
    let uid = unsafe { libc::getuid() };
    let e = red.run("console.log(process.getuid());\n");
    assert_eq!(e.stdout.trim(), uid.to_string());
    let corto = Runner::new(&preload, crate::test_util::id_runner(), Duration::from_millis(800)).with_sandbox(Sandbox::PermisosYRed);
    let e = corto.run("while (true) {}\n");
    assert_eq!(e.status, Status::Timeout);
    drop((red, solo, corto, l));
    let _ = std::fs::remove_dir_all(&b);
}
