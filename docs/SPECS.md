# Completion spec reference

Tools are described by declarative, indentation-based `.riga` files. The bundled specs in `data/specs/` are original and compiled into the binary. A bundled spec applies only when its command is an executable on the shell's PATH, so uninstalled tools get no suggestions. No third-party completion data or implementation source was copied. See [research references](INTEGRATION-RESEARCH.md). This repository's code and specs use the MIT license.

## Format

Each line is one statement, indented two spaces per nesting level (tabs are an error), optionally followed by a `"description"`. A `#` starts a comment only at the beginning of a token (line start or after whitespace), so `{a#b}` is an ordinary choice.

```text
command docker "Manage containers"
  flag --help|-h persistent "Print usage"
  flag --log-level|-l {debug,info,warn,error,fatal} "Set the logging level"
  command run "Create and run a container"
    flag --interactive|-i "Keep STDIN open"
    flag --rm excludes=--restart "Remove the container when it exits"
    flag --restart {no,always,on-failure,unless-stopped} "Restart policy"
    flag --ip <address> requires=--network "IPv4 address"
    flag --network <network> "Connect to a network"
    arg <image> stop "Image to run"
    arg [<command>] repeat "Command and arguments"
```

| Statement | Form | Attributes |
| --- | --- | --- |
| `command` | `command NAME[\|ALIAS...]` | `nobundle`: no multi-flag short bundles at this level (attached values such as `-Cdir` still work) |
| `flag` | `flag NAME[\|ALIAS...] [VALUE]` | `repeat`, `persistent` (also visible in subcommands), `chdir` (needs `<directory>`; later paths resolve from it), `requires=--a,--b` (hidden until all are used), `excludes=--a` (symmetric) |
| `arg` | `arg VALUE` | `repeat` (absorbs the remaining positionals; must be last), `stop` (after this arg, stop option parsing and spec suggestions; any later args document usage only and are not completed) |

### Values

- `<file>` and `<directory>` use the filesystem provider.
- `<git-...>`, `<npm-...>`, `<cargo-...>` and the other built-in tokens read local data. See [Built-in value sources](#built-in-value-sources).
- Any other `<label>` is free text.
- `{a,b,c}` lists choices.
- `[VALUE]` makes a value optional. An optional flag value is only recognized attached (`--color=auto`) and never consumes the next word. On an arg, `[]` is documentation only, since positionals always fill args in order.

### Names and validation

- Depth 0 is exactly one `command`, the executable name. Only commands have children.
- Command names must not start with `-`. Flag names must.
- `--x` is long. `-x` is short and bundleable as `-sb`, with a value-taking flag ending the bundle (`-mmsg`). `-xyz` is a single-dash long name that is never split. A value-taking one also accepts an attached `=` value (`-var-file=dev.tfvars`), while `-x=v` keeps `=v` as the value.
- Names use only alphanumerics and `-_.:+@%/`, so an option such as curl's `-#` cannot be declared (the curl spec offers `--progress-bar` instead).
- `--x` is long. `-x` is short and bundleable as `-sb`, with a value-taking flag ending the bundle (`-mmsg`). `-xyz` is a single-dash long name that is never split.
- Load errors: duplicate spellings in one scope or statement, duplicate attributes or references, unresolved `requires`/`excludes` references, unknown keywords or attributes, a description without a statement, and empty choices. They are reported as `file:line: message`, sorted by line, at most 100 per file. Children of a rejected `command` are skipped rather than reported.
- A leading UTF-8 byte order mark is ignored.
- Limits: nesting depth 32, at most 64 names per statement and 128 characters per name, descriptions of at most 200 characters without control or bidirectional characters (choices are checked the same way), 1 MiB per file. The number of flags, subcommands and choices is not capped. Completion ranks lightweight candidates and builds at most 64 suggestions.
- Choices have no per-choice descriptions. Test expectation labels cannot contain spaces.

## How completion follows the words

- A word matching a subcommand descends, only before the current command has consumed a positional. Visible flags are the current command's plus ancestors' `persistent` flags. Other ancestor flags are valid only before descending (`git -C dir status`, but not `git status -C dir`).
- Used non-`repeat` flags disappear under every spelling. `git status --short ` still offers `--branch`. `git commit -m "First" ` offers another `--message`.
- Flag values: choices are offered as `value` suggestions (`git log --pretty=on` gives `--pretty=oneline`). `<file>`/`<directory>` use the filesystem. Other text values offer nothing.
- Positionals fill `arg`s in order. Choice args offer only their choices. `<directory>` args offer only directories. Text and Git args still offer paths after their own suggestions. A leaf without declared args accepts any positionals. An unknown word after a command that has subcommands but no args stops spec suggestions.
- `--` ends option parsing. Later args are still completed by type. Unknown options stop spec suggestions because their arity is unknown.
- Filesystem suggestions are suppressed after a completed `chdir` value (such as `git -C`) beginning with `~`, whose original quoting cannot be established from the decoded preceding tokens. Use an absolute path for that case.
- The spec is chosen by the first word only, so wrappers such as `sudo git ...`, `env ... git` or `command git` get no spec completion.
- Specs cannot run code, shell generators or callbacks. Dynamic values come only from the built-in sources below.

## Bundled specs and tools

Only the specs bundled in `data/specs/` are loaded; they are compiled into the binary and apply only to commands found on the shell's PATH. User-supplied specs are not supported. Error messages escape control and bidirectional characters from the spec text they quote, so a spec cannot send terminal escape sequences through `check-spec`.

```sh
riga check-spec path/to/tool.riga     # validate specific files; exits 1 on any error or failed test
riga complete --cwd /tmp "git diff --color="   # dry run: label, kind, description, insert per line
riga doctor                           # lists the bundled specs active for the current PATH
```

### Tests

Any line beginning with `#test` (after indentation) must be a valid test, so near misses such as `#testing` are errors rather than silent comments. A line `#test "<buffer>" => +shown -absent` completes the quoted buffer (cursor at the end, `\"` and `\\` escapes, trailing spaces significant) with only that spec. The filesystem and PATH are never read, even for absolute paths. The first character of each expectation is its sign, so `---short` means `--short` must be absent.

```text
#test "git status -sb -" => +--show-stash ---branch --b
```

`cargo test` runs every bundled `#test`. `riga check-spec` runs them for any spec.

## Built-in value sources

Sources read local files and never run the tool, except tmux, ffmpeg and docker (see [Background tool runs](#background-tool-runs)). They use default locations under the home directory the daemon resolves for the user. Rules shared by all of them:

- Only regular files are read. FIFOs, devices and directories with a manifest's name are ignored, so they cannot hang the worker.
- Invalid UTF-8 is read lossily. Non-UTF-8 directory entries are skipped.
- Names containing control or bidirectional characters, empty names, and names over 256 characters are dropped. Duplicates keep the first description. Descriptions are cut to 100 characters, and one with control or bidirectional characters is blanked.
- Names are sorted, then capped, so an oversized source yields the first names alphabetically.
- The daemon does not see the shell's environment (other than PATH). `$KUBECONFIG`, `$CARGO_HOME`, `$RUSTUP_HOME`, `$OLLAMA_MODELS`, `$UV_TOOL_DIR`, `$HOMEBREW_CACHE`, `$AWS_CONFIG_FILE` and `$AWS_SHARED_CREDENTIALS_FILE` are not followed, nor are `--kubeconfig`, `--manifest-path`, `--project`, `--prefix`, `--extensions-dir`, `--user-data-dir` or `cdk --output` in the buffer. tmux `-L`/`-S` sockets are not followed. Docker uses the daemon's own environment and the current context in `~/.docker/config.json`; the shell's `$DOCKER_HOST`/`$DOCKER_CONTEXT` and `-H`/`--context` in the buffer are not followed.
- "Paths" below means file system paths are offered next to the values. Git tokens do so for positionals only, not flag values.

| Token(s) | Lists | Read from | Paths | Not followed |
| --- | --- | --- | --- | --- |
| `<git-branch>`, `<git-ref>`, `<git-remote>` | Branches, refs, remotes | The repository files (see [Git refs](#git-refs)) | Positionals | Fetching, `git` itself |
| `<npm-script>` | `scripts` names | Nearest `package.json` (1 MiB, 5,000 names) | No | Running npm |
| `<npm-dependency>` | `dependencies`, `devDependencies`, `optionalDependencies`, `peerDependencies` names | Nearest `package.json` | No | The registry |
| `<npm-bin>` | Installed binaries | Nearest `node_modules/.bin` | Positionals | |
| `<npm-workspace>` | Workspace directories | `workspaces` in `package.json` (literal paths and a trailing `/*`, each with a `package.json`; up to 500) | No | Other glob forms |
| `<cargo-package>`, `<cargo-lock-package>`, `<cargo-bin>`, `<cargo-example>`, `<cargo-test>`, `<cargo-bench>`, `<cargo-feature>`, `<cargo-dependency>`, `<cargo-profile>` | Packages, targets, features, dependencies, profiles | Nearest `Cargo.toml`, then the workspace root and its members (up to 500), and `Cargo.lock` | No | `--manifest-path`, running cargo or build scripts |
| `<cargo-installed>` | Installed crates | `~/.cargo/.crates.toml` | No | A custom `$CARGO_HOME` or `--root` |
| `<py-dependency>`, `<py-group>`, `<py-extra>`, `<py-script>` | Dependencies, groups, extras, scripts | Nearest `pyproject.toml` | `<py-script>` positionals | `--project` |
| `<uv-tool>` | Installed uv tools | `~/.local/share/uv/tools` | No | `$UV_TOOL_DIR` |
| `<brew-package>`, `<brew-formula>` | All available formulae and casks, or formulae | `~/Library/Caches/Homebrew/api/*_names.txt` | No | `$HOMEBREW_CACHE` |
| `<brew-installed>`, `<brew-installed-formula>` | Installed formulae and casks, or formulae | `Cellar` and `Caskroom` under the Homebrew prefix | No | `--prefix` |
| `<brew-pinned>`, `<brew-tap>` | Pinned formulae, taps | `var/homebrew/pinned`, `Library/Taps` under the prefix | No | |
| `<kube-context>`, `<kube-cluster>`, `<kube-user>`, `<kube-namespace>` | Names from the kubeconfig | `~/.kube/config` (1 MiB, 5,000 names). No cluster is contacted. | No | `$KUBECONFIG`, `--kubeconfig` |
| `<ollama-model>` | Models, named as `ollama list` shows them | `~/.ollama/models/manifests` | No | `$OLLAMA_MODELS` |
| `<rustup-toolchain>` | Toolchains, plus `stable`, `beta`, `nightly` | `~/.rustup/toolchains` | No | `$RUSTUP_HOME` |
| `<rustup-target>`, `<rustup-component>` | Targets, components | Each toolchain's `lib/rustlib/components` | No | `$RUSTUP_HOME` |
| `<vscode-extension>` | Installed extensions (skips `.obsolete` entries) | `~/.vscode/extensions` | No | `--extensions-dir` |
| `<vscode-profile>` | Profile names | `userDataProfiles` in the first 1 MiB of VS Code's `storage.json` | No | `--user-data-dir` |
| `<aws-profile>`, `<aws-sso-session>` | Profiles (`[default]`, `[profile X]`, and credentials `[X]`), `[sso-session X]` names | `~/.aws/config`, `~/.aws/credentials` (1 MiB each, 5,000 names). AWS is never contacted. | No | `$AWS_CONFIG_FILE`, `$AWS_SHARED_CREDENTIALS_FILE` |
| `<cdk-stack>` | CloudFormation stack artifacts of the last synth, by display name (else artifact id) | `cdk.out/manifest.json` in the working directory (8 MiB, 5,000 names). The CDK app is never run. | No | `--output`, `cdk.json` `output`, nested stage assemblies |
| `<tmux-session>`, `<tmux-window>`, `<tmux-pane>`, `<tmux-buffer>` | Names on the default tmux server | Background run of `tmux list-... -F ...` | No | `-L`, `-S` |
| `<ffmpeg-encoder>`, `<ffmpeg-video-encoder>`, `<ffmpeg-audio-encoder>`, `<ffmpeg-subtitle-encoder>`, `<ffmpeg-format>`, `<ffmpeg-video-filter>`, `<ffmpeg-audio-filter>` | What the local ffmpeg build supports | Background run of `ffmpeg -hide_banner -encoders` / `-formats` / `-filters` | No | |
| `<docker-container>`, `<docker-running-container>`, `<docker-image>` | All containers, running containers (with status), tagged images (with size) | Background run of `docker ps -a` / `docker ps` / `docker images` with `--format` | No | The shell's `$DOCKER_HOST`, `-H`, `--context` |
| `<terraform-workspace>` | `default` plus local workspaces (the selected one is labelled) | `terraform.tfstate.d/*` and `.terraform/environment` in the working directory (or `-chdir`) | No | Workspaces of remote backends, `$TF_WORKSPACE` |
| `<terraform-var-file>` | `*.tfvars` and `*.tfvars.json` files | The working directory and its immediate subdirectories | No | Deeper or parent directories |
| `<terraform-resource>` | Resource and instance addresses (`module.x.aws_subnet.s["a"]`) | `resources` in the selected workspace's local state, `terraform.tfstate` or `terraform.tfstate.d/<name>/terraform.tfstate` (16 MiB, 5,000 names). Terraform is never run, since it starts provider plugins. | No | Remote backends, `-state`, a custom local backend `path` |

The Homebrew prefix is the one of the `brew` on the shell's PATH, else `/opt/homebrew`, then `/usr/local`.

### Git refs

Git values are read directly from the repository files, never by running `git`. Riga finds the repository from the working directory (or a `git -C` directory), following `.git` files of worktrees and submodules, and lists `refs/` plus `packed-refs`.

- `<git-branch>`: local branches (the checked-out one is labelled "Current branch"), plus branches that exist only on a remote, offered by the short name `git switch`/`git checkout` turn into a tracking branch.
- `<git-ref>`: local branches, remote-tracking branches (`origin/main`) and tags.
- `<git-remote>`: remotes from `.git/config` and fetched remote refs.

Riga never fetches. Branches pushed by others appear after your own `git fetch` or `git pull`. Suggestions are capped at 20,000 refs and a 16 MiB `packed-refs`. Names with control or bidirectional characters are hidden. The bundled Git spec uses these for `checkout`, `switch`, `branch`, `rebase`, `restore --source`, `log`, `diff`, `fetch`, `pull` and `push`.

### Background tool runs

`<tmux-*>`, `<ffmpeg-*>` and `<docker-*>` run fixed, read-only list commands (`tmux list-sessions -F ...`, `ffmpeg -hide_banner -encoders`, `docker ps -a --format ...`, ...) on a background thread. The executable is found on the shell's PATH and started without a shell, in its own process group.

- A run is killed after 3 seconds. Output is bounded (1 MiB, 5,000 lines). The cache holds at most 64 entries.
- Completion never waits. Results are cached: 5 seconds for tmux and docker, 10 minutes for ffmpeg. If the Docker daemon is not reachable, runs fail: names from the last successful run stay listed until a run succeeds again, and none are shown if it never did.
- A failed run keeps the previous values and is retried after 15 seconds.
- The first request may show no values. The popup refreshes when a run finishes.
