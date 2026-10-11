# Usage details

Setup details and behavior that the [README](../README.md) only summarizes. The spec format is in [SPECS.md](SPECS.md).

## Trying it

From an iTerm2 zsh prompt in a checkout (after `cargo build --release`):

```zsh
export RIGA_BIN="$PWD/target/release/riga"
"$RIGA_BIN" doctor
"$RIGA_BIN" serve &
source "$PWD/shell/riga.zsh"
```

Type `git st`, `cd ` or the prefix of a command. The server must remain running. Return keeps its ordinary execute behavior. The popup does not take keyboard focus or handle mouse clicks. When the popup is hidden, each installed key delegates to its previous ZLE widget or macro.

While `riga serve` is running, a terminal icon appears in the macOS menu bar. Click it to see the running status or choose **Quit Riga**. The icon disappears when the server exits. Diagnostic commands (`doctor` and `probe`) do not add an icon.

## Permissions

The first Automation query may prompt to control iTerm2. In **System Settings → Privacy & Security → Accessibility**, allow the application responsible for launching Riga (for this terminal workflow, iTerm2). Restart Riga if necessary. Under **Automation**, allow that launcher to control iTerm2. These are separate permissions.

`riga doctor` reports Accessibility trust and socket health. `riga probe` prints only the actual pane ID and caret geometry, or a suppression reason. It does not print command text.

Do not grant permissions blindly to an unrelated executable. A future packaged build should use a stable signed application identity. This development slice runs directly from an existing terminal.

## Persistent installation and removal

Make the release binary available as `riga` on your zsh PATH, or set an absolute `RIGA_BIN` in your shell configuration before the managed block. Then:

```sh
./scripts/install.sh                         # updates ${ZDOTDIR:-$HOME}/.zshrc
./scripts/install.sh --rc /path/to/test.zshrc # optional explicit destination
./scripts/install.sh --uninstall            # removes only the managed block
```

Installation and removal are idempotent, preserve permissions, and retain a first-install `.riga-backup`. The installer refuses symlink destinations and malformed managed blocks. It does not start a daemon or alter system permissions. Keep the checkout at its installed path, or rerun the installer after moving it. Source after other ZLE customizations. Existing additive ZLE hooks, including iTerm2 shell integration, remain installed.

In an already open shell, `riga-unload` removes Riga's hooks, closes its connection, and restores keys. Stop the background `riga serve` process with the shell job's normal `kill %job-number`, or Ctrl-C when running it in the foreground. Restarting handles a stale socket safely.

## Keys and socket

Set keys **before** sourcing, using zsh `bindkey` notation:

```zsh
RIGA_KEY_NEXT='^N'
RIGA_KEY_PREV='^P'
RIGA_KEY_ACCEPT='^I'
RIGA_KEY_DISMISS='^G'
```

Use distinct bindings. Changes apply to `emacs` and `viins`. Vi command mode retains its bindings. Avoid rebinding these keys after sourcing, or reload Riga to capture the new defaults. `RIGA_SOCKET` overrides the Unix socket path for both processes. Its parent directory must belong to you and have mode `0700`.

## Architecture

- `shell/riga.zsh` uses additive `line-init`, `line-pre-redraw` and `line-finish` hooks. It transmits `BUFFER`, character `CURSOR`, cwd, shell PATH, an independent shell session ID, iTerm2 session identity, and monotonically increasing revisions. Builtin `zsocket`, `syswrite` and `zle -F` keep keystrokes free of subprocesses. One native helper makes the socket nonblocking when connecting. Backpressure disconnects instead of blocking typing.
- `src/main.rs` owns IPC and routing. Each shell session is bound to one connection. A worker coalesces pending requests per session. Stale results, ended edits, old revisions and cross-connection updates are discarded. Only sessions still editing can show results. Acceptance performs a fresh foreground/pane check before sending a replacement. See [PROTOCOL.md](PROTOCOL.md).
- `src/completion.rs` contains terminal-independent Rust providers, a conservative shell lexer and character-index replacement ranges. It enumerates the shell's PATH and filesystem entries and interprets declarative `.riga` specs (`src/spec.rs` parses and validates them). Filesystem work occurs off the AppKit thread, never in ZLE. Executable listings refresh after 30 seconds or when the effective PATH changes. Dotfiles appear only for dot prefixes. `cd` restricts to directories. Exact matches rank first, then values the command expects (choices, Git refs), then spec commands and flags, then directories and files.
- `src/macos.rs` uses raw Rust bindings to AppKit, Accessibility, CoreFoundation and the Objective-C runtime. It verifies iTerm2 is frontmost, reads its focused AX text area's selected range and `AXBoundsForRange`, and checks focus and pane identity again. Native NSAppleScript retrieves the active session UUID via iTerm2's documented AppleScript interface. That UUID matches the suffix of `ITERM_SESSION_ID`, identifying the current window/tab/pane. Queries refresh approximately every 70 ms and on selection/acceptance. The panel keeps the same verified AX focus stable during refresh and hides on a changed focus element, unsupported AX responses, lost focus or ended editing.

AppKit points are converted from AX's top-left coordinate system into the primary screen's AppKit coordinate space. The popup is clamped to the caret display's visible frame and can flip above the caret near the display edge. No guessed or fixed fallback is used. See [INTEGRATION-RESEARCH.md](INTEGRATION-RESEARCH.md) for upstream sources and additional setup requirements.

## Parser limits and safety

The parser handles ordinary commands, quoted/escaped paths, Unicode scalar offsets, command separators and middle-of-token replacement. It deliberately suppresses substitutions, unquoted globbing, redirects, assignments and other unsupported syntax rather than inventing replacement ranges. It does not resolve aliases or functions, named-user tilde expansion, shell expansions or remote sessions. tmux panes are not supported. Non-UTF-8 filenames are skipped. New files in a directory appear on the next edit. Executable discovery has a 30-second cache and each directory scan is capped at 20,000 entries. Results are capped at 64 with seven rows visible.

No command buffers are logged or persisted. Local IPC is protected by a `0700` parent and `0600` socket. Other processes running as your own user are within this prototype's trust boundary. Shell frames are capped at 64 KiB. Buffers above 16,384 characters suppress completion. Disconnection reconnects at the next command prompt. The server is manually started. There is no background agent installation, update service, cloud account, telemetry or global key monitor.

## Not yet verified live

Live checks passed on one Retina display with iTerm2 3.7.2. Still unverified: multiline paste, reverse search, visual widths of non-CJK Unicode, focus changes into settings/search and terminal text selection, full-screen apps, multiple displays (mixed scaling, negative origins, fullscreen Spaces), and revoking permissions mid-edit. Focus tracking polls about every 70 ms, so focus transitions have finite detection latency.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
RIGA_TEST_BIN="$PWD/target/debug/riga" python3 scripts/test_shell.py
python3 scripts/test_daemon.py
```

The PTY test exercises a real zsh with controlled IPC replies. It can use the native nonblocking helper. The daemon test exercises the real process. macOS GUI/TCC and multi-display behavior need manual checks in a disposable iTerm2 session; automated tests cannot establish visual behavior. Do not press Return when testing acceptance.
