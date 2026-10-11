# Riga

Terminal autocomplete for macOS, zsh and iTerm2 only: a Rust daemon plus `shell/riga.zsh`, with a native popup at the terminal caret. Everything runs locally: no accounts, cloud, telemetry or AI. Only bundled specs (`data/specs/`) are supported, never user-supplied ones.

## Invariants

- ZLE is the only source of command state; never reconstruct buffers from global key events.
- Acceptance edits the buffer and cursor only and never executes the command.
- Typing never waits on completion: no synchronous subprocesses or blocking socket writes in shell hooks.
- Drop stale results (wrong connection, revision or pane). Show the popup only for the verified active iTerm2 pane with real caret geometry; never fall back to a fixed position.
- Don't hide and reshow the panel on every refresh (it flashes).
- Shell bindings work normally while the popup is hidden; loading, unloading and reloading restore widgets and macros.
- Replacement ranges are Unicode character offsets, not bytes. Inserted words are shell-escaped.
- Value sources read files and never run the tool. Never run `git`: repository config and hooks can execute code, and its data is readable from files.
- Never log or persist command buffers. IPC stays private and messages bounded.

## Checks

```sh
cargo test
cargo clippy --all-targets -- -D warnings
RIGA_TEST_BIN="$PWD/target/debug/riga" python3 scripts/test_shell.py
python3 scripts/test_daemon.py
```

Tests can't prove popup placement or focus behavior. Check UI changes in a disposable iTerm2 session, and say in the commit message what was checked live and what wasn't. Test the installer with temporary shell config files, never the user's `.zshrc`.

Keep `README.md` (short overview), `docs/USAGE.md` (including "Not yet verified live"), `docs/SPECS.md` and `docs/PROTOCOL.md` accurate.
