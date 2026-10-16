# Contributing

Thanks for your interest in difuzz. Most of the documentation (`MANUAL.md`, `tests/README.md`)
is in Spanish.

## Requirements

- Rust and cargo (checked with rustc 1.98.0; the crate does not declare `rust-version`).
- `node` on `PATH` (checked with Node v24.14.1).
- Linux on x86_64 or aarch64 (the `--sandbox` seccomp filter only exists there). The
  `--sandbox-red` tests need unprivileged user namespaces; without them they fail, and
  `DIFUZZ_SIN_USERNS=1` skips them.
- For the scripts in `tests/` and `scripts/`: bash, `rg` and `fd`; `scripts/comparar-binarios.sh`
  and `scripts/subir-oxc.sh` also need `eza` and `sd` (>= 1.0), `scripts/medir-serie.sh` and
  `scripts/subir-oxc.sh` need `git`, and `scripts/armar-corpus-real.py` needs `python3`. Some
  scripts list their tools in a `Dependencias:` or `Herramientas:` line of their header; not
  all do.
- Only for the C1 audit of `tests/muestra/`: `npm ci` in `probe/`.

## Build

```sh
cargo build --release && cargo build --profile oraculo
```

This builds two binaries: `target/release/difuzz` (with debug assertions) and
`target/oraculo/difuzz` (the oracle, without them). The golden, acceptance and codegen checks
expect both; rebuild them together.

## Check your change

```sh
cargo test
cargo clippy --all-targets -- -D warnings
tests/golden/verificar.sh --jobs 4                  # last line: comparaciones=N fallas=0 salida=DIR
tests/aceptacion/rechequeos/verificar.sh --jobs 4
tests/codegen/verificar.sh --jobs 4
cargo test muestra::c1 -- --ignored --nocapture     # needs npm ci in probe/
```

The end-to-end checks run node many times with timeouts, so they are sensitive to machine load:
on a busy machine, use a lower `--jobs` and re-run a failure before trusting it.

For a refactor that must not change any result, compare the old and new binaries case by case:

```sh
scripts/comparar-binarios.sh --jobs 4 --corpus tests/golden/corpus BIN_BASE BIN_NEW OUT_DIR "oxc b" "swc a"
```

## Updating oxc

`CORPUS=<dir> scripts/subir-oxc.sh VERSION OUT_DIR` moves the `oxc_*` crates and `oxc-minify` in
`probe/` to a new version together, in a temporary worktree and without committing. It needs a
corpus (`CORPUS` or `DIFUZZ_CORPUS`; without one it exits with 2). It builds both binaries for
each version, runs the tests, golden, acceptance and the comparison harness, and leaves a report of
which findings appear, disappear or change, plus a patch to apply. The procedure is in
`scripts/subir-oxc.md` (in Spanish); `scripts/subir-oxc.sh --help` lists the variables.

## Labels in comments

Code comments, test data and many commit messages cite labels from difuzz's development log,
which is not part of the repository:

- items of the improvement series (`10.2`, `11.2c`);
- findings of the reviews of each series (`R1-H05`, `M30`, `I3`, `H4`, `B-F2`, ...);
- short tags of earlier work (`b1-b`, `t-principio`, `n-ruido`).

They are kept as stable tags, and the comment next to each one says what the change does and why.
Labels from different reviews can repeat (there is more than one `I3`); the surrounding text tells
them apart. For a numbered item, `git log --grep` often finds its commit.

## Commits and pull requests

- Use conventional commits (`fix: ...`, `feat: ...`, `test: ...`, `docs: ...`, `chore: ...`).
- This project is developed with AI assistance. If you used AI tools for your contribution, say
  so in the pull request description.

## License

difuzz is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
