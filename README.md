# Riga

Riga is an autocomplete popup for **zsh in iTerm2 on macOS**, written in Rust. A small native panel appears next to the terminal caret and offers suggestions while you edit a command. Everything runs locally. There are no accounts, no cloud services, no telemetry and no AI. It is a working prototype.

## What it completes

- Executables on your PATH, and files and directories (including `cd`).
- Bundled completion specs for common CLI tools (see `data/specs/`): subcommands, flags, choices and descriptions.
- Real names where a spec asks for them, such as branches, scripts, packages and contexts. Most are read from local files; a few come from read-only list commands run in the background.

## Requirements

macOS, iTerm2, zsh, Xcode Command Line Tools and Rust (edition 2024). There are no Cargo dependencies.

## Quick start

```zsh
cargo build --release
export RIGA_BIN="$PWD/target/release/riga"
"$RIGA_BIN" doctor
"$RIGA_BIN" serve &
source "$PWD/shell/riga.zsh"
```

Type `git st` or `cd ` and the popup appears. The server must keep running. While it does, a menu bar icon shows its status.

Riga needs two macOS permissions for the app that launches it (iTerm2): **Accessibility**, to find the caret, and **Automation**, to ask iTerm2 for the active pane. `riga doctor` checks setup. `riga probe` prints the real caret geometry, or why the popup is suppressed.

### Install permanently

```sh
./scripts/install.sh              # adds a managed block to ~/.zshrc
./scripts/install.sh --uninstall  # removes it
```

This is idempotent and does not start the server. Details are in [docs/USAGE.md](docs/USAGE.md).

## Keys

| Key | Action |
| --- | --- |
| Ctrl-N / Ctrl-P | Next / previous suggestion |
| Tab | Insert the selected suggestion |
| Ctrl-G | Dismiss |

Insertion edits the buffer only. Riga never runs the command. When the popup is hidden, the keys keep their normal behavior. To rebind, set `RIGA_KEY_NEXT`, `RIGA_KEY_PREV`, `RIGA_KEY_ACCEPT` and `RIGA_KEY_DISMISS` (zsh `bindkey` notation, for example `'^N'`) before sourcing `shell/riga.zsh`.

## Specs

Completion specs are bundled with Riga (`data/specs/*.riga`, compiled into the binary) and apply only to commands on your PATH. User-supplied specs are not supported.

`riga check-spec FILE...` validates spec files and runs their `#test` lines when developing bundled specs. The full format and the list of built-in value sources are in [docs/SPECS.md](docs/SPECS.md).

## Privacy and safety

- Command buffers are never logged or stored.
- The socket is private (`0700` directory, `0600` socket) and messages are size-limited.
- Accepting a suggestion never submits or executes the command.
- Completion reads files and never runs project code. The only commands Riga runs are fixed, read-only list commands for a few tools, in the background with a 3 second limit.

## Limitations

- macOS, zsh and iTerm2 only. No tmux panes, remote sessions or other terminals.
- The popup appears only when the caret position and active pane can be verified. Focus tracking polls, so there is a short delay after a change.
- Aliases, shell functions, wrappers such as `sudo git` and complex shell syntax get no completion.
- Specs cannot run code.
- Value sources use default locations and ignore environment variables such as `$KUBECONFIG`.
- Not yet verified: extra displays and mixed scaling, fullscreen, reverse search.

## Documentation

- [docs/USAGE.md](docs/USAGE.md): setup details, permissions, architecture, limits.
- [docs/SPECS.md](docs/SPECS.md): spec format and value sources.
- [docs/PROTOCOL.md](docs/PROTOCOL.md): shell to daemon protocol.
- [docs/INTEGRATION-RESEARCH.md](docs/INTEGRATION-RESEARCH.md): iTerm2 and Accessibility research.

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
RIGA_TEST_BIN="$PWD/target/debug/riga" python3 scripts/test_shell.py
python3 scripts/test_daemon.py
```

## License

MIT. See [LICENSE](LICENSE).
