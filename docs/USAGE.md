# Usage details

Setup details and behavior that the [README](../README.md) only summarizes. The spec format is in [SPECS.md](SPECS.md).

## Trying it

From an iTerm2 zsh prompt in a checkout (after `cargo build --release`):

```zsh
export RIGA_BIN="$PWD/target/release/riga"
"$RIGA_BIN" doctor
source "$PWD/shell/riga.zsh"
```

Sourcing `riga.zsh` starts `riga serve` in the background when no server answers on the socket, and logs to `$TMPDIR/riga-serve.log`. Because it is started from iTerm2, iTerm2's Accessibility and Automation permissions cover it. Starting several shells at once is safe: extra servers exit on a lock in the socket directory. The server then keeps running after the shell closes. The first prompt of the shell that started it may show no popup until the server is listening. Set `RIGA_AUTOSTART=0` before sourcing to start the server yourself. Riga deliberately installs no launchd agent: a server started by launchd would not run under iTerm2's permissions.

Type `git st`, `cd ` or the prefix of a command. Return keeps its ordinary execute behavior. The popup does not take keyboard focus or handle mouse clicks. When the popup is hidden, each installed key delegates to its previous ZLE widget or macro.

While `riga serve` is running, a terminal icon appears in the macOS menu bar. Click it to see the running status or choose **Quit Riga**. The icon disappears when the server exits. Diagnostic commands (`doctor` and `probe`) do not add an icon.

## Natural language to command

On by default. To turn it off, set `RIGA_AI=0` in the environment of `riga serve` (export it in `.zshrc` before `riga.zsh` so an auto-started server inherits it, or run `RIGA_AI=0 "$RIGA_BIN" serve &` yourself); `#` lines then show no popup. It is a daemon setting rather than a shell setting because the daemon runs the model, and keeping it there leaves the shell protocol unchanged. Requirements: Apple Intelligence enabled on a supported Mac, and the `fm` license accepted once with `sudo fm license`. If the model is unavailable, the popup says so.

A line that starts with `# ` followed by at least three words is a request. About 400 ms after the last keystroke, Riga asks the on-device model (`/usr/bin/fm respond --model system --greedy`, with guided generation) for up to three commands. Only one run is in flight at a time, and a run is killed after 8 seconds. The prompt contains the request, the current directory's name and the names of installed tools that have bundled specs. It never contains file contents or history. The prompt goes to `fm` on stdin, so it doesn't appear in `ps`. Prompts and replies are kept only in a 32-entry in-memory cache and are never logged.

Before a suggestion is shown, Riga's own lexer must accept it (no newlines, substitutions, unquoted globs or redirects). Every command in it must be on the shell's PATH, and a command with a bundled spec may use only the subcommands and flags that spec knows. Suggestions containing `sudo`, `rm -rf`, `dd`, `mkfs`, `diskutil` or a pipe into a shell are dropped. Accepting replaces the whole line, as typed, without running it. While the model runs, the popup shows one row, "Asking the on-device model…". If the model doesn't answer (Apple Intelligence off, `fm` license not accepted, timeout) or no suggestion passes the checks, that row changes to say so. Accepting one of these rows only closes the popup.

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

Installation and removal are idempotent, preserve permissions, and retain a first-install `.riga-backup`. The installer refuses symlink destinations and malformed managed blocks. It does not alter system permissions; the sourced script starts the server on demand (see above). Keep the checkout at its installed path, or rerun the installer after moving it. Source after other ZLE customizations. Existing additive ZLE hooks, including iTerm2 shell integration, remain installed.

In an already open shell, `riga-unload` removes Riga's hooks, closes its connection, and restores keys. Stop the server with **Quit Riga** in its menu bar icon; the next shell that loads Riga starts it again. Restarting handles a stale socket safely.

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
- `src/completion.rs` contains terminal-independent Rust providers, a conservative shell lexer and character-index replacement ranges. It enumerates the shell's PATH and filesystem entries and interprets declarative `.riga` specs (`src/spec.rs` parses and validates them). Filesystem work occurs off the AppKit thread, never in ZLE. Executable listings refresh after 30 seconds or when the effective PATH changes. Dotfiles appear only for dot prefixes. `cd` restricts to directories. Exact matches rank first, then values the command expects (choices, Git refs), then spec commands and the command's own flags, then directories and files, then flags inherited from parent commands.
- `src/macos.rs` uses raw Rust bindings to AppKit, Accessibility, CoreFoundation and the Objective-C runtime. It verifies iTerm2 is frontmost, reads its focused AX text area's selected range and `AXBoundsForRange`, and checks focus and pane identity again. Native NSAppleScript retrieves the active session UUID via iTerm2's documented AppleScript interface. That UUID matches the suffix of `ITERM_SESSION_ID`, identifying the current window/tab/pane. Queries refresh approximately every 70 ms and on selection/acceptance. The panel keeps the same verified AX focus stable during refresh and hides on a changed focus element, unsupported AX responses, lost focus or ended editing.

AppKit points are converted from AX's top-left coordinate system into the primary screen's AppKit coordinate space. The popup is clamped to the caret display's visible frame and can flip above the caret near the display edge. No guessed or fixed fallback is used. See [INTEGRATION-RESEARCH.md](INTEGRATION-RESEARCH.md) for upstream sources and additional setup requirements.

## Parser limits and safety

The parser handles ordinary commands, quoted/escaped paths, Unicode scalar offsets, command separators and middle-of-token replacement. It deliberately suppresses substitutions, unquoted globbing, redirects, assignments and other unsupported syntax rather than inventing replacement ranges. It does not resolve aliases or functions, named-user tilde expansion, shell expansions or remote sessions. tmux is supported only as a tool to complete (`tmux ...` commands), not as a pane host. Non-UTF-8 filenames are skipped. New files in a directory appear on the next edit. Executable discovery has a 30-second cache and each directory scan is capped at 20,000 entries. Results are capped at 128 with seven rows visible.

No command buffers are logged or persisted. Local IPC is protected by a `0700` parent and `0600` socket. Other processes running as your own user are within this prototype's trust boundary. Shell frames are capped at 64 KiB. Buffers above 16,384 characters suppress completion. Disconnection reconnects at the next command prompt. The server is started by the shell integration, not a launch agent. There is no background agent installation, update service, cloud account, telemetry or global key monitor. AI suggestions are on-device only.

## Not yet verified live

Live checks passed on one Retina display with iTerm2 3.7.2. Still unverified: multiline paste, reverse search, visual widths of non-CJK Unicode, focus changes into settings/search and terminal text selection, full-screen apps, multiple displays (mixed scaling, negative origins, fullscreen Spaces), and revoking permissions mid-edit. Focus tracking polls about every 70 ms, so focus transitions have finite detection latency. AI suggestions were checked live only on macOS 27.0.1 with one Apple Silicon Mac. Requests are expected in English. Still unverified: Apple Intelligence turned off or still downloading. AWS CLI data (`aws` services and operations generated from the installed CLI) was tested against extracted CLI data and fake installs, not against a real official package (`/usr/local/aws-cli/v2/<version>/dist`) or Homebrew install on this Mac, and the popup refresh after the background read of a large service such as `ec2` was not watched live. Server auto-start is untested live: that Accessibility and Automation still apply to an auto-started server, and that it survives closing its shell and quitting iTerm2.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
RIGA_TEST_BIN="$PWD/target/debug/riga" python3 scripts/test_shell.py
python3 scripts/test_daemon.py
```

The PTY test exercises a real zsh with controlled IPC replies. It can use the native nonblocking helper. The daemon test exercises the real process. macOS GUI/TCC and multi-display behavior need manual checks in a disposable iTerm2 session; automated tests cannot establish visual behavior. Do not press Return when testing acceptance.
