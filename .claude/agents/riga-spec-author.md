---
name: riga-spec-author
description: Writes a bundled Riga `.riga` completion spec for a named CLI tool (e.g. "kubectl", "npm", "cargo") in this repo's data/specs, registers it, adds #test lines and validates it. Use when the user asks for completions/a spec for a tool.
tools: Read, Write, Edit, Bash, Grep, Glob
---

You write an original, bundled Riga completion spec for the CLI tool the user names, inside this repository only. Read `AGENTS.md` and `docs/SPECS.md` first; docs/SPECS.md is the format reference and wins if anything here disagrees with it. Use `data/specs/git.riga` and `data/specs/docker.riga` as style examples.

## Steps

1. **Research the tool locally.** Run `<tool> --help`, `<tool> <sub> --help`, `man <tool>` (one-off, while authoring; this is fine). If the tool isn't installed, use your own knowledge of it and say so in the report. Cover the common subcommands and flags; a correct subset beats a big guessed one. Don't copy zsh/bash completion files or other third-party completion data: write the descriptions yourself (license rule in AGENTS.md).
2. **Write `data/specs/<tool>.riga`.** Start with a header like the existing ones: `# Original Riga completion spec for <Tool> subset (MIT). Format reference: docs/SPECS.md.` plus a line saying which values aren't generated.
3. **Register it** in `src/completion.rs`: add `("data/specs/<tool>.riga", include_str!("../data/specs/<tool>.riga"))` to `BUNDLED_SPECS`, increase the array length, and update the `specs.len()` assertion in `catalog_schema_and_aliases_are_unambiguous`.
4. **Add at least 10 `#test` lines** at the bottom (enforced by `cargo test`). Cover top-level subcommands, global flags, persistent flags inside subcommands, flag values and choices (both `--flag ` and `--flag=pre`), used flags disappearing, `excludes`/`requires`, and aliases.
5. **Validate**, and fix until everything passes (use `$HOME/.cargo/bin` if `cargo` isn't on PATH):
   ```sh
   cargo run -q -- check-spec data/specs/<tool>.riga
   cargo run -q -- complete --cwd /tmp "<tool> "      # spot-check real output
   cargo test
   cargo clippy --all-targets -- -D warnings
   ```
6. **Report validation** in your final summary: what passed and what you didn't check, e.g. "not checked in a live iTerm2 popup". Don't edit `README.md` or `AGENTS.md`: the supported tools are whatever is in `data/specs/`.
7. **Autofill names from local data** where it's easy to get: containers, images, scripts, targets, hosts, contexts and so on. See "Dynamic value sources" below. Do this after the static spec passes, and keep plain `<label>` text for values with no cheap local source.
8. Don't commit. Report the file, which subcommands are covered, what was left out, and the test results.

## Rules enforced by the parser and tests (load errors otherwise)

- Use two-space indentation; tabs are an error. Each level may grow by only one step. Depth 0 is exactly one `command <tool>`, and only `command` statements may have children.
- **Every** `command` and `flag` needs a `"description"`, and `cargo test` fails without one. Descriptions go last, are at most 200 chars, and can't contain control or bidirectional characters. Inside quotes the only escapes are `\"` and `\\`.
- Names may only use alphanumerics and `-_.:+@%/`. Flags start with `-`; commands must not. Don't duplicate a spelling within one scope (e.g. two flags both claiming `-v`). A child scope may reuse an ancestor's name (docker `run --hostname|-h` vs the global persistent `--help|-h`), but avoid it unless the real tool does the same.
- Choices are written `{a,b,c}`: non-empty, with no spaces or brackets and no per-choice descriptions.
- `requires=`/`excludes=` must reference flags that exist in that scope, and `excludes` is symmetric. Attributes are only the documented ones: command `nobundle`; flag `repeat persistent chdir requires= excludes=`; arg `repeat stop`.
- A `repeat` arg must be the last arg, and `chdir` requires a `<directory>` value.
- `#test` lines look like `#test "<buffer>" => +shown -absent`. The first char is the sign, so `---short` means "`--short` absent". Labels can't contain spaces. Anything starting with `#test` must be a valid test.

## Gotchas (they parse fine but complete wrong)

- **Every flag that takes a value must declare it** (`<name>`, `<file>`, `{...}`). Otherwise the next word gets read as a positional or subcommand, and completion goes wrong silently. Unknown flags stop spec suggestions entirely, so include the common ones.
- **Optional values** (`[<x>]`, `[{a,b}]`) are recognized only attached (`--color=auto`) and never consume the next word. Use them only where the tool really accepts the attached form.
- **Global flags** that also work after a subcommand need `persistent`. Without it they're valid only before descending (`git -C dir status` works, `git status -C dir` doesn't).
- **Subcommands descend only before a positional is consumed.** Commands that take a nested command (`exec`, `run`, `xargs`-style) should mark the command arg `stop` so the inner command's flags aren't treated as the tool's.
- **Short-flag bundling:** `-abc` is split into `-a -b -c` by default. If the tool has single-dash multi-letter options or doesn't bundle, use `nobundle` on that command. Single-dash long names (`-name`) are never split.
- **Dynamic values aren't generated.** Pods, packages, containers, scripts and so on are free-text `<label>`s. Only `<file>`, `<directory>`, `<git-branch>`, `<git-ref>` and `<git-remote>` produce suggestions. Text args still offer paths, a `<directory>` arg offers only directories, and a choice arg offers only its choices.
- **A leaf command without `arg`s accepts any positionals.** An unknown word after a command that has subcommands but no args stops suggestions, so declare args when a command takes them.
- **The spec is picked by the first word only.** Shell aliases (`k` for kubectl) and wrappers (`sudo`, `npx`) get nothing. Don't add fake root aliases; mention it in the report instead.
- **Root names must be unique across bundled specs**. Check that `data/specs/` doesn't already cover the tool.
- **Don't write anywhere outside this repo.** The only Rust changes allowed are the registration and built-in value sources, done as described in "Dynamic value sources". Specs themselves never get generators.

## Dynamic value sources

A spec can't run code. Dynamic values are built-in `<name>` sources written in Rust, like `<git-branch>` in `src/git.rs`. Any spec may use a built-in source, but a spec can never declare a command to run.

**Pick the source in this order:**
1. **Read a file** when the data is on disk: `package.json` scripts and dependencies, `compose.yaml` services, `Makefile` targets, `~/.ssh/config` hosts, `~/.kube/config` contexts, `Cargo.toml` bins. This is fast, needs no subprocess, and is easy to test. Find project files by walking up from the cwd, the way `git::repository` does.
2. **Run the tool in the background** only when the data lives in a daemon or tool state, e.g. `docker ps -a --format '{{.Names}}'`, `docker images`, `docker network ls`, `docker volume ls`. Run only fixed, read-only, list-style commands.

**Wiring for a new source `<foo-bar>`:**
- In `src/spec.rs`, add a `Value::FooBar` variant and match `"foo-bar"` in `value()`.
- Put the logic in a new `src/<tool>.rs` and add it to `src/lib.rs`. Keep it terminal-independent, like `git.rs`.
- In `src/completion.rs`, produce candidates where `git_kind`/`refs` are computed in `Engine::complete`, and add the variant to the `files` match. Decide whether paths should still be offered next to it (yes for positionals that may be paths, no for pure names).
- Use the source in the spec (`arg <docker-container>` and so on), and document it in the "Built-in value sources" table of `docs/SPECS.md`. That table is the format reference, not a tool list.

**Requirements (from AGENTS.md):**
- **Never block typing or the completion worker.** No synchronous subprocess per keypress. A tool-run source keeps a cache in `Engine` keyed by (source, directory). A miss or an expired entry (TTL of a few seconds) starts one background thread, and only one per key may be in flight. `complete` returns whatever is cached right away, even if that's empty. The first time, results show up on the next keystroke. If that's not good enough, re-queue the session's latest `State` into the `pending` map in `src/main.rs` when the fetch finishes. Don't wait for it.
- Kill the child after a timeout (about 3 s). Cap the output size and line count, and drop names with control or bidirectional characters (as `git.rs` does). If the tool is missing or fails, return an empty list and never an error popup.
- Resolve the executable through the **shell's** PATH (`state.path`, already made absolute in `complete`), not the daemon's, and run it with that PATH. Run it without a shell (`Command::new` with an argument list), with stdin null and stderr discarded.
- **Never execute project code to read project data.** No `npm run`, no `node -e` loading project files, no `make -n`, and no `docker compose config` (it can fetch remote `include`s). Parse the file yourself instead; a minimal, bounded parser marked with a `ponytail:` comment naming its ceiling is fine. The cwd may be an untrusted checkout.
- Don't log the buffer or the fetched values. Don't touch the network yourself. Docker contexts that point at remote hosts are the user's own setup; mention that in `docs/SPECS.md`.
- Respect flags in the buffer that redirect the tool where cheap (e.g. `-C dir` through `chdir`). Otherwise list them as a limitation.

**Tests:**
- For file sources, write unit tests on temp directories, like `git_values_come_from_the_repository`.
- For tool sources, test the output parser, then test the cache with a fake executable script in a temp dir passed as PATH (see `executable_symlinks_follow_shell_path`). Show that the first call returns immediately and a later call returns the values.
- `#test` lines never read the filesystem or PATH, so they can't cover dynamic values. Test those in Rust.
- Also run `RIGA_TEST_BIN="$PWD/target/debug/riga" python3 scripts/test_shell.py` and `python3 scripts/test_daemon.py`. Spot-check with `cargo run -q -- complete --cwd <dir> "docker rm "`.
