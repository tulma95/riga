#!/bin/sh
# Installs only a managed source block; never starts a service or changes TCC.
set -eu
operation=install
rc=${ZDOTDIR:-$HOME}/.zshrc
while [ "$#" -gt 0 ]; do
  case "$1" in
    --uninstall) operation=uninstall; shift ;;
    --rc) [ "$#" -ge 2 ] || { echo '--rc requires a path' >&2; exit 2; }; rc=$2; shift 2 ;;
    *) echo 'Usage: scripts/install.sh [--uninstall] [--rc PATH]' >&2; exit 2 ;;
  esac
done
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
# Refuse a symlink to avoid changing its target unexpectedly.
[ ! -L "$rc" ] || { echo "Refusing symlink: $rc (use --rc with its target)" >&2; exit 1; }
mkdir -p "$(dirname -- "$rc")"
if [ ! -e "$rc" ]; then
  [ "$operation" = install ] || { echo "No integration installed in $rc"; exit 0; }
  : > "$rc"
fi
# A malformed block is not safe to rewrite automatically.
awk '
  /^# >>> riga >>>$/ { if (inside) exit 1; inside=1; next }
  /^# <<< riga <<<$/{ if (!inside) exit 1; inside=0; next }
  END { if (inside) exit 1 }
' "$rc" || { echo 'Malformed Riga block; repair it before running this installer.' >&2; exit 1; }
tmp=$(mktemp "${rc}.riga.XXXXXX")
trap 'rm -f "$tmp"' EXIT HUP INT TERM
# Preserve permissions, and retain a one-time backup of the pre-install file.
cp -p "$rc" "$tmp"
if [ "$operation" = install ] && [ ! -e "${rc}.riga-backup" ]; then cp -p "$rc" "${rc}.riga-backup"; fi
awk '/^# >>> riga >>>$/ {skip=1; next} /^# <<< riga <<<$/{skip=0; next} !skip {print}' "$rc" > "$tmp"
if [ "$operation" = install ]; then
  # Single-quote shell paths, including embedded apostrophes.
  quoted=$(printf '%s' "$root/shell/riga.zsh" | sed "s/'/'\\\\''/g")
  printf '# >>> riga >>>\n[ ! -r '\''%s'\'' ] || source '\''%s'\''\n# <<< riga <<<\n' "$quoted" "$quoted" >> "$tmp"
fi
mv "$tmp" "$rc"
trap - EXIT HUP INT TERM
printf '%s: %s\n' "$operation" "$rc"
