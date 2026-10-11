#!/bin/zsh
# Build Riga, (re)start its server in the background, and load the zsh integration.
# `source ./run.sh` does everything in the current shell; `./run.sh` only builds and starts the server.
root=${${(%):-%x}:A:h}
bin=$root/target/release/riga
log=${${TMPDIR:-/tmp}%/}/riga-serve.log
(cd "$root" && cargo build --release) || return 1 2>/dev/null || exit 1
pkill -f "^$bin serve" && sleep 0.2
nohup "$bin" serve >>"$log" 2>&1 &!
"$bin" doctor
if [[ $ZSH_EVAL_CONTEXT == *:file* ]]; then
  export RIGA_BIN=$bin
  source "$root/shell/riga.zsh"
  print "Riga loaded. Server log: $log"
else
  print "Server started (log: $log). Load it in your shell with:\n  export RIGA_BIN='$bin'; source '$root/shell/riga.zsh'"
fi
