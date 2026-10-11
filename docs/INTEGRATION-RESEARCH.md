# iTerm2 integration research

Investigated on 2026-10-07. These are implementation references, not evidence that a UI test passed.

## Caret geometry

The [upstream Amazon Q Developer CLI caret implementation](https://github.com/aws/amazon-q-developer-cli-autocomplete/blob/main/crates/macos-utils/src/caret_position.rs) obtains the focused Accessibility element, reads `AXSelectedTextRange`, and requests `AXBoundsForRange`. It supports extending an empty range by one character, rejects larger selections, and rejects an empty rectangle. Riga uses this API approach; no upstream implementation or completion catalog was copied.

iTerm2's [PTYTextView implementation](https://github.com/gnachman/iTerm2/blob/master/sources/TerminalView/PTYTextView.m) exposes selected text ranges and range bounds. Its coordinate conversion explicitly uses the top of the primary screen as the Accessibility origin. AppKit windows use a different vertical coordinate system; conversion must use the primary screen's height, including when the caret is on a display with a negative desktop origin. A zero-size rectangle or missing range is a failure, never a reason to position at a fixed point.

Current upstream source is supporting evidence, not a guarantee about the installed iTerm2 3.7.2 build. The initial live probe was denied Accessibility permission (`AXError -25211`). After the user enabled iTerm2 in Accessibility, the native Riga probe returned real caret bounds, and live popup checks passed on the available display. See [USAGE.md](USAGE.md#not-yet-verified-live) for remaining limitations.

## Exact pane identity

The inspected text view does not provide a documented session UUID through `AXIdentifier`. Matching window titles or using whichever shell updated most recently would be unsafe for tabs and split panes.

iTerm2's [documented AppleScript interface](https://iterm2.com/documentation-scripting.html) provides the current window's current session and its unique ID. The installed app's scripting dictionary (`/Applications/iTerm.app/Contents/Resources/iTerm2.sdef`) maps `unique ID` to the session GUID. A read-only query of `unique ID of current session of current window` succeeded locally. The shell's `ITERM_SESSION_ID` includes the session GUID; window/tab ordinal prefixes must not be treated as permanent identities. Automation permission is separate from Accessibility permission.

Foreground application, focused Accessibility element, and pane identity must agree before displaying suggestions. AppleScript identity lookup must run outside the shell's input path. Polling introduces a finite detection interval; it cannot establish an absolute guarantee that no stale frame is ever visible during a focus transition. Treat the live tab/pane switching checklist as a release gate.

## Documented alternatives when AX fails

1. First distinguish denied permission from an unsupported attribute. Grant Accessibility to the app or responsible launcher in System Settings → Privacy & Security → Accessibility, restart the process, and rerun the diagnostic while iTerm2 is focused. Grant Automation access to iTerm2 if prompted. Permission denial is not proof that iTerm2 lacks caret geometry.
2. iTerm2's [Python API](https://iterm2.com/python-api/session.html) exposes session UUIDs, containing tabs/windows, grid sizes, and screen updates. [ScreenContents.cursor_coord](https://iterm2.com/python-api/screen.html) provides cursor cell coordinates. These are not automatically a global AppKit caret rectangle: a fallback must also validate pane content origins, font metrics, padding, scroll position, and display scaling. Window bounds plus a guessed cell size are insufficient.
3. To investigate that API, enable the Python API in iTerm2 Settings → General → Magic, create a script using Scripts → New Python Script, and install the Python runtime when iTerm2 prompts. The [official running guide](https://iterm2.com/python-api/tutorial/running.html) describes running scripts from the Scripts menu or an environment with the `iterm2` package. External scripts require an authorization prompt. AutoLaunch is optional. This is a documented investigation route, **not an implemented or validated fallback** in Riga; the runtime remains Rust plus zsh.

The normal [iTerm2 shell integration](https://iterm2.com/documentation-shell-integration.html) offers prompt and command lifecycle metadata. It does not replace ZLE's editable buffer or establish a global pixel caret position. Keep it installed if already in use. Riga's hooks use the [ZLE hook/widget APIs](https://zsh.sourceforge.io/Doc/Release/Zsh-Line-Editor.html), rather than global keyboard-event reconstruction.

Callbacks and user-defined shell generators are not supported.

The former Git-only TSV catalog was replaced by general `.riga` specs (bundled specs for Git, Docker and other tools) describing nested commands, option aliases, value types and choices, repeatability, persistent flags, requires/excludes rules, and positional args. The native completion provider interprets preceding ZLE tokens to retain command context across options and arguments, including short-option bundles and attached values. This is local parsing of declarative data. Value sources read local files directly; the only subprocesses are fixed, read-only `tmux`, `ffmpeg` and `docker` list commands run on a background thread (`src/background.rs`), never in the shell. Specs have no shell hooks, user-defined generators or callbacks, and nothing changes pane and caret tracking. See the [spec format](SPECS.md) for supported behavior and checks.

## Menu bar presence

The server retains a native [NSStatusItem](https://developer.apple.com/documentation/appkit/nsstatusitem) for its lifetime, using the system terminal symbol and a menu with running status and Quit Riga. Apple documents that [NSStatusBar does not retain status items](https://developer.apple.com/documentation/appkit/nsstatusbar/statusitem(withlength:)); Riga owns and releases the item explicitly. Accessory activation policy permits the menu without a Dock entry. The existing nonactivating completion panel still preserves terminal focus. Only `serve` creates the status item; diagnostics stay transient.
