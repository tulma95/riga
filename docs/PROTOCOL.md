# Local IPC protocol v1

A persistent Unix stream carries UTF-8 newline-delimited frames, at most 65,536 bytes. Fields are separated by TAB. `%`, TAB, LF and CR are escaped as `%25`, `%09`, `%0A`, `%0D` in text fields. Decode once; literal `%0A` therefore survives. IDs contain ASCII letters, digits, `-_.:` only and are limited to 255 bytes. Revisions are unsigned 64-bit integers. Cursor and replacement ranges count Unicode scalar values, matching zsh's multibyte editor, not UTF-8 bytes.

Shell → daemon:

- `S SESSION PANE REV CURSOR CWD BUFFER PATH` starts/updates a live edit. CWD, BUFFER and PATH are escaped. PANE is `ITERM_SESSION_ID`; SESSION is unique to the shell process. PATH is captured from that shell, not the daemon environment.
- `K NEXT PREV ACCEPT DISMISS` reports the connection's key bindings in escaped zsh `bindkey` notation, each at most 16 characters. Sent once after connecting; the popup footer shows them. A connection that never sends it gets no key hints.
- `E SESSION REV` ends an edit and hides all its suggestions.
- `A SESSION REV next|prev|accept|dismiss` operates on current suggestions only after a fresh native focus/pane/geometry check.

Daemon → shell:

- `P REV COUNT` reports visible suggestion count; zero restores ordinary key behavior.
- `I REV START END INSERT` requests replacement of the half-open character range `[START, END)` with escaped INSERT. It never includes Return or an execute operation.

The first state binds SESSION to the connection. Subsequent states must preserve pane identity and strictly increase revision. Completion results must match the current revision and editing state. The shell independently checks revision, BUFFER, CURSOR, cwd and PATH before applying a reply. Moving the cursor, typing, pasting or navigating history invalidates acceptance before it can touch a newer buffer. All queueing is ephemeral; no buffers are persisted.
