# Security

## difuzz runs untrusted code

difuzz executes every corpus case under Node.js: the original and the minified program, plus
extra runs for confirmations and re-checks. If the corpus is not yours (npm packages, code from
the web, cases someone else generated), a run executes that code as the user running difuzz.

## Sandbox

By default node runs **without a sandbox**: a case can read and write whatever your user can,
spawn processes and use the network.

- `--sandbox`: every node run uses the Node 24 permission model (`--permission`). Reads are
  allowed only for the preload, the case (and difuzz's module wrapper) and the case's working
  directory; writes only for that directory and difuzz's control channel. No child processes, workers, native addons or WASI,
  and `node:sqlite` is disabled (`--no-experimental-sqlite`). A seccomp filter (Linux x86_64 and
  aarch64 only; on other architectures `--sandbox` refuses to start) lets node send signals only
  to itself and change only its own priority.
- `--sandbox-red` (Linux only, no root, needs user namespaces): `--sandbox` plus user, network
  and mount namespaces. Only a private loopback is reachable, `/run` is hidden behind a tmpfs,
  and the system's unix sockets outside `/run` are covered with `/dev/null`. If it cannot be set
  up, difuzz exits with 1 before running anything.
- `scripts/barrido-real.sh` always uses `--sandbox-red`.
- Every node run has a timeout (`--timeout-ms`, 5000 ms by default).

**Use `--sandbox-red` for any corpus you do not trust.**

## Known limits

- The Node permission model is **not a barrier against hostile code**. With `--sandbox` alone
  it does not block the network or `connect` to a unix socket, so a case can reach D-Bus,
  systemd-resolved or any other service with a socket.
- With `--sandbox-red`, unix sockets created after node starts, and those with spaces in their
  path, stay reachable. The rest of the filesystem is still visible to the operating system;
  only the Node permission model restricts it, not the namespace.
- In Node 24, `node:sqlite` does not go through the permission model (that is why the sandbox
  disables it). The other Node modules have not been audited against the permission model.
- Runs that need the inspector use `--allow-inspector`, which lets the program open an
  inspector session in its own process; Node warns that this may invalidate the permission
  model. Opening an inspector port is blocked.
- The `repro_original:` / `repro_minificado:` commands written to `meta.txt` do not include the
  sandbox flags: running them by hand runs the case without the sandbox.

The Sandbox section of `README.md` (in Spanish) has the full details and measurements.

## Reporting a vulnerability

If you find a vulnerability in difuzz itself (for example, a way for a corpus case to get past
`--sandbox-red` beyond the limits listed above), please report it privately through GitHub's
private vulnerability reporting ("Report a vulnerability" in the repository's Security tab)
instead of opening a public issue.
<!-- VERIFICAR: private vulnerability reporting has to be enabled in Settings > Security before publishing. -->

Miscompilations that difuzz finds in a minifier are not vulnerabilities in difuzz: report them
to that minifier's project.
