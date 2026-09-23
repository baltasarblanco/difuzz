#!/bin/bash
# Mide cada commit de la serie "confianza" (HEAD~5..HEAD de ~/difuzz) en
# ~/difuzz-main2, contra ~/swc-main (42e4f7e98b) y corpus-4b42.
# Base de comparacion: ~/difuzz-main2/out-main (8 div, 5 conocidas, 524 observados).
# Escribe out-c1..out-c6, build-c*.log, run-c*.log y stats-c*.txt en ~/difuzz-main2.
set -u
cd ~/difuzz-main2 || exit 1
[ -f Cargo.toml.antes-serie ] || cp Cargo.toml Cargo.toml.antes-serie
for n in 1 2 3 4 5 6; do
  c=$(git -C ~/difuzz rev-parse --short "HEAD~$((6 - n))")
  echo "=== commit $n ($c) $(git -C ~/difuzz log -1 --format=%s "$c")"
  rm -rf src && git -C ~/difuzz archive "$c" src | tar -x
  # git archive pone la fecha del commit: sin esto cargo puede no ver el cambio.
  find src -type f -exec touch {} +
  # Cargo.toml de ese commit + el [patch] hacia ~/swc-main (+ swc_ecma_visit desde el 3)
  git -C ~/difuzz show "$c:Cargo.toml" > Cargo.toml
  printf '\n' >> Cargo.toml
  sed -n '/^\[patch.crates-io\]/,/^$/p' ~/difuzz-main/Cargo.toml >> Cargo.toml
  if grep -q '^swc_ecma_visit' Cargo.toml; then
    echo 'swc_ecma_visit           = { path = "/home/baltasarblanco/swc-main/crates/swc_ecma_visit" }' >> Cargo.toml
  fi
  if ! cargo build --release > "build-c$n.log" 2>&1; then
    echo "  NO COMPILA: ver ~/difuzz-main2/build-c$n.log"; tail -20 "build-c$n.log"; break
  fi
  if ! grep -q 'Compiling difuzz' "build-c$n.log"; then
    echo "  NO RECOMPILO difuzz: el binario seria el del commit anterior"; break
  fi
  if grep -qi 'not used' "build-c$n.log"; then
    echo "  PATCH NO USADO:"; grep -i 'not used' "build-c$n.log"; break
  fi
  rm -rf "out-c$n"
  ./target/release/difuzz corpus-4b42 --out "out-c$n" 2> "run-c$n.log" > "stats-c$n.txt"
  cat "stats-c$n.txt"
done
