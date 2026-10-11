//! Completion providers have no terminal or UI dependencies. Ranges count Unicode scalars, as ZLE does.
use crate::{
    ai, aws_cli, background,
    protocol::State,
    sources::Env,
    spec::{self, Arg, Spec, Value, ValueSpec},
};
use std::{
    collections::HashSet,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
const MAX_RESULTS: usize = 128;
/// Kind of an informational row (AI progress); accepting it never edits the buffer.
pub const STATUS: &str = "status";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    pub kind: &'static str,
    pub description: String,
    pub insert: String,
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Context {
    pub prefix: String,
    pub start: usize,
    pub end: usize,
    pub previous: Vec<String>,
    pub expand_tilde: bool,
}
/// A deliberately conservative lexer: suppress expansions, substitutions and unsupported shell syntax.
pub fn context(buffer: &str, cursor: usize) -> Option<Context> {
    let cs: Vec<char> = buffer.chars().collect();
    if cursor > cs.len() {
        return None;
    }
    let mut tokens: Vec<(usize, usize, String)> = vec![];
    let mut start = None;
    let mut value = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut previous = vec![];
    for i in 0..=cs.len() {
        if i == cs.len() {
            if let Some(s) = start {
                tokens.push((s, i, value.clone()));
            }
            break;
        }
        let c = cs[i];
        if escaped {
            if c != '\n' {
                value.push(c);
            }
            escaped = false;
            continue;
        }
        if c == '\\'
            && quote != Some('\'')
            && (quote != Some('"') || cs.get(i + 1).is_some_and(|next| "$`\"\\\n".contains(*next)))
        {
            start.get_or_insert(i);
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                if q == '"' && "$`".contains(c) {
                    return None;
                }
                value.push(c);
            }
            continue;
        }
        if c == '\'' || c == '"' {
            start.get_or_insert(i);
            quote = Some(c);
            continue;
        }
        if "$`(){}<>*?[!#".contains(c) {
            return None;
        }
        if c.is_whitespace() || ";|&".contains(c) {
            if let Some(s) = start.take() {
                tokens.push((s, i, std::mem::take(&mut value)));
            }
            if ";|&\n".contains(c) && i < cursor {
                tokens.clear();
            }
        } else {
            start.get_or_insert(i);
            value.push(c);
        }
    }
    for (s, e, v) in tokens {
        if s <= cursor && cursor <= e {
            // Recompute prefix independently so a cursor at token start never inherits the preceding token.
            let raw: String = cs[s..cursor].iter().collect();
            let p = unquote_prefix(&raw)?;
            return Some(Context {
                prefix: p,
                start: s,
                end: e,
                previous,
                expand_tilde: cs[s] == '~',
            });
        }
        if e < cursor {
            previous.push(v);
        }
    }
    Some(Context {
        prefix: String::new(),
        start: cursor,
        end: cursor,
        previous,
        expand_tilde: false,
    })
}
fn unquote_prefix(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut quote = None;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && quote != Some('\'')
            && (quote != Some('"') || chars.peek().is_some_and(|n| "$`\"\\\n".contains(*n)))
        {
            let next = chars.next()?;
            if next != '\n' {
                out.push(next);
            }
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                out.push(c);
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
        } else {
            out.push(c);
        }
    }
    Some(out)
}
pub fn quote_word(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || "_@%+:,./-".contains(c))
    {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
const BUNDLED_SPECS: [(&str, &str); 18] = [
    (
        "data/specs/git.riga",
        include_str!("../data/specs/git.riga"),
    ),
    (
        "data/specs/docker.riga",
        include_str!("../data/specs/docker.riga"),
    ),
    (
        "data/specs/node.riga",
        include_str!("../data/specs/node.riga"),
    ),
    (
        "data/specs/npm.riga",
        include_str!("../data/specs/npm.riga"),
    ),
    (
        "data/specs/brew.riga",
        include_str!("../data/specs/brew.riga"),
    ),
    (
        "data/specs/cargo.riga",
        include_str!("../data/specs/cargo.riga"),
    ),
    (
        "data/specs/code.riga",
        include_str!("../data/specs/code.riga"),
    ),
    (
        "data/specs/ffmpeg.riga",
        include_str!("../data/specs/ffmpeg.riga"),
    ),
    (
        "data/specs/kubectl.riga",
        include_str!("../data/specs/kubectl.riga"),
    ),
    (
        "data/specs/npx.riga",
        include_str!("../data/specs/npx.riga"),
    ),
    (
        "data/specs/ollama.riga",
        include_str!("../data/specs/ollama.riga"),
    ),
    (
        "data/specs/rustup.riga",
        include_str!("../data/specs/rustup.riga"),
    ),
    (
        "data/specs/tmux.riga",
        include_str!("../data/specs/tmux.riga"),
    ),
    ("data/specs/uv.riga", include_str!("../data/specs/uv.riga")),
    (
        "data/specs/terraform.riga",
        include_str!("../data/specs/terraform.riga"),
    ),
    (
        "data/specs/curl.riga",
        include_str!("../data/specs/curl.riga"),
    ),
    (
        "data/specs/aws.riga",
        include_str!("../data/specs/aws.riga"),
    ),
    (
        "data/specs/cdk.riga",
        include_str!("../data/specs/cdk.riga"),
    ),
];
fn bundled_specs() -> Vec<Spec> {
    BUNDLED_SPECS
        .into_iter()
        .filter_map(|(label, text)| spec::parse(label, text).ok())
        .collect()
}
/// Interprets preceding words against a spec: scope, used flags, a pending flag value and positionals.
struct SpecContext<'a> {
    spec: Option<&'a Spec>,
    command: usize,
    /// Flags visible in `command`, computed once per scope.
    visible: Vec<usize>,
    used: HashSet<usize>,
    pending: Option<usize>,
    enabled: bool,
    options: bool,
    positionals: usize,
    cwd: Option<PathBuf>,
    /// A word the spec doesn't know (flag or subcommand) disabled it.
    unknown: bool,
    /// The next word belongs to an unknown option of an `open` command.
    skip_value: bool,
}
impl<'a> SpecContext<'a> {
    fn new(spec: Option<&'a Spec>, previous: &[String], cwd: &Path) -> Self {
        let mut result = Self {
            spec,
            command: 0,
            visible: spec.map(|s| s.visible_flags(0)).unwrap_or_default(),
            used: HashSet::new(),
            pending: None,
            enabled: spec.is_some(),
            options: true,
            positionals: 0,
            cwd: Some(cwd.into()),
            unknown: false,
            skip_value: false,
        };
        let Some(spec) = spec else {
            return result;
        };
        for word in previous.iter().skip(1) {
            if !result.enabled {
                break;
            }
            if std::mem::take(&mut result.skip_value) && !(word.starts_with('-') && word != "-") {
                continue;
            }
            // In an `open` command words the spec lacks (services of an uninstalled CLI) are
            // skipped, not fatal: only its persistent flags stay meaningful.
            let open = spec.commands[result.command].open;
            if let Some(flag) = result.pending.take() {
                if spec.flags[flag].chdir {
                    result.change_directory(word);
                }
                continue;
            }
            if result.options && word == "--" {
                result.options = false;
            } else if result.options && word.starts_with('-') && word != "-" {
                if let Some((flags, pending)) = result.option(word) {
                    if pending.is_none()
                        && let Some((_, directory, flag)) = result.attached_value(word)
                        && spec.flags[flag].chdir
                    {
                        result.change_directory(directory);
                    }
                    result.used.extend(flags);
                    result.pending = pending;
                } else if open {
                    // Its arity is unknown: take a following plain word as its value.
                    result.skip_value = !word.contains('=');
                } else {
                    // An unknown option might consume the next word: do not guess its arity.
                    result.enabled = false;
                    result.unknown = true;
                }
            } else {
                let command = &spec.commands[result.command];
                let descend = result.options && result.positionals == 0;
                if let Some(&child) = command.child_names.get(word).filter(|_| descend) {
                    result.command = child;
                    result.visible = spec.visible_flags(child);
                } else if open {
                    result.positionals += 1;
                    result.visible.retain(|&f| spec.flags[f].persistent);
                } else if !command.children.is_empty() && command.args.is_empty() {
                    result.enabled = false;
                    result.unknown = true;
                } else {
                    result.enabled = !result.arg().is_some_and(|arg| arg.stop);
                    result.positionals += 1;
                }
            }
        }
        result
    }
    /// The declared arg for the next positional; a trailing `repeat` arg absorbs the rest.
    fn arg(&self) -> Option<&'a Arg> {
        let args = &self.spec?.commands[self.command].args;
        args.get(self.positionals)
            .or_else(|| args.last().filter(|arg| arg.repeat))
    }
    fn change_directory(&mut self, directory: &str) {
        if directory.is_empty() {
            return;
        }
        // Previous tokens no longer retain quote provenance, so ~ could be literal or expanded.
        if directory.starts_with('~') {
            self.cwd = None;
        } else if Path::new(directory).is_absolute() {
            self.cwd = Some(directory.into());
        } else if let Some(cwd) = &mut self.cwd {
            *cwd = cwd.join(directory);
        }
    }
    fn takes_value(&self, flag: usize) -> Option<&'a ValueSpec> {
        self.spec?.flags[flag].value.as_ref()
    }
    /// Parse complete options, including -sb, -mmessage and --message=message.
    /// Returns the used flags and the flag whose required value is the next word.
    fn option(&self, word: &str) -> Option<(Vec<usize>, Option<usize>)> {
        let spec = self.spec?;
        if let Some(flag) = spec.flag(self.command, word) {
            let pending = self.takes_value(flag).is_some_and(|v| !v.optional);
            return Some((vec![flag], pending.then_some(flag)));
        }
        if word.starts_with("--") {
            let (name, _) = word.split_once('=')?;
            let flag = spec.flag(self.command, name)?;
            return self.takes_value(flag).map(|_| (vec![flag], None));
        }
        if let Some((_, _, flag)) = self.single_dash_value(word) {
            return Some((vec![flag], None));
        }
        let mut used = vec![];
        let mut letters = word.strip_prefix('-')?.chars().peekable();
        while let Some(letter) = letters.next() {
            let flag = spec.flag(self.command, &format!("-{letter}"))?;
            used.push(flag);
            if let Some(value) = self.takes_value(flag) {
                let pending = letters.peek().is_none() && !value.optional;
                return Some((used, pending.then_some(flag)));
            }
            if spec.commands[self.command].nobundle {
                return None;
            }
        }
        (!used.is_empty()).then_some((used, None))
    }
    /// `-name=value` for a value-taking single-dash long flag (`terraform -var-file=x`). Short flags
    /// keep `-m=x` as the value `=x`, since the name must be a declared multi-letter spelling.
    fn single_dash_value<'w>(&self, word: &'w str) -> Option<(&'w str, &'w str, usize)> {
        let (name, value) = word.split_once('=')?;
        if name.starts_with("--") || name.chars().count() <= 2 {
            return None;
        }
        let flag = self.spec?.flag(self.command, name)?;
        self.takes_value(flag)?;
        Some((&word[..name.len() + 1], value, flag))
    }
    /// Split a current option's attached value, preserving its spelling during replacement.
    fn attached_value<'w>(&self, prefix: &'w str) -> Option<(&'w str, &'w str, usize)> {
        let spec = self.spec?;
        if prefix.starts_with("--") {
            let (name, value) = prefix.split_once('=')?;
            let flag = spec.flag(self.command, name)?;
            self.takes_value(flag)?;
            return Some((&prefix[..name.len() + 1], value, flag));
        }
        if let Some(found) = self.single_dash_value(prefix) {
            return Some(found);
        }
        // `-cp` may be a single-dash long flag being typed, not `-c` with value `p`.
        let typing_flag = self.visible.iter().any(|&f| {
            let names = &spec.flags[f].names;
            names.iter().any(|n| n.starts_with(prefix))
        });
        if typing_flag {
            return None;
        }
        for (offset, letter) in prefix.strip_prefix('-')?.char_indices() {
            let flag = spec.flag(self.command, &format!("-{letter}"))?;
            if self.takes_value(flag).is_some() {
                let end = 1 + offset + letter.len_utf8();
                return (end < prefix.len()).then_some((&prefix[..end], &prefix[end..], flag));
            }
            if spec.commands[self.command].nobundle {
                return None;
            }
        }
        None
    }
    fn hidden(&self, flag: usize) -> bool {
        let f = &self.spec.unwrap().flags[flag];
        (!f.repeat && self.used.contains(&flag))
            || f.excludes.iter().any(|e| self.used.contains(e))
            || !f.requires.iter().all(|r| self.used.contains(r))
    }
}
struct Candidate<'a> {
    label: &'a str,
    kind: &'static str,
    description: &'a str,
    names: &'a [String],
    /// Attached option spelling (`--format=`) prefixed to an inserted value.
    option: &'a str,
    /// A persistent flag of an ancestor command, ranked after the current command's own entries.
    inherited: bool,
}
/// Order within the popup after exact matches: values a command expects, then its commands and flags.
fn tier(kind: &str) -> u8 {
    match kind {
        "value" | "ref" => 1,
        "command" | "flag" => 2,
        "directory" => 3,
        _ => 4,
    }
}
fn described(description: &str, names: &[String]) -> String {
    if names.len() > 1 {
        format!("{description} ({})", names.join(", "))
    } else {
        description.into()
    }
}
/// Which `aws` spec applies, see `Engine::aws_spec`.
enum AwsSpec {
    Curated,
    /// The CLI's data is being read; completion uses this spec meanwhile (the curated one if none).
    Pending(Option<Arc<Spec>>),
    Ready(Arc<Spec>),
}
pub struct Engine {
    executables: Vec<String>,
    scanned: Instant,
    path: String,
    home: PathBuf,
    specs: Vec<Spec>,
    filesystem: bool,
    /// Bundled specs apply only to commands found on the shell's PATH.
    installed_only: bool,
    runs: background::Runs,
    /// Services and operations read from the installed AWS CLI's data.
    aws: aws_cli::Cli,
    /// Natural language to command; on unless the daemon's environment has `RIGA_AI=0`.
    ai: Option<ai::Ai>,
}
impl Default for Engine {
    fn default() -> Self {
        let mut engine = Self::new(
            std::env::var("PATH").unwrap_or_default(),
            std::env::var_os("HOME").unwrap_or_default().into(),
        );
        engine.installed_only = true;
        engine.ai = (std::env::var("RIGA_AI").as_deref() != Ok("0")).then(ai::Ai::default);
        engine
    }
}
impl Engine {
    pub fn new(path: String, home: PathBuf) -> Self {
        let aws = aws_cli::Cli::default();
        Self {
            executables: vec![],
            scanned: Instant::now() - Duration::from_secs(60),
            path,
            home,
            specs: bundled_specs(),
            filesystem: true,
            installed_only: false,
            runs: background::Runs::with_custom(aws.exec()),
            aws,
            ai: None,
        }
    }
    fn scan_path(&mut self) {
        if self.scanned.elapsed() < Duration::from_secs(30) {
            return;
        }
        let mut seen = HashSet::new();
        for dir in std::env::split_paths(&self.path) {
            if !dir.is_absolute() {
                continue;
            }
            if let Ok(entries) = fs::read_dir(dir) {
                for e in entries.take(20000).flatten() {
                    if let Ok(m) = fs::metadata(e.path())
                        && m.is_file()
                        && m.permissions().mode() & 0o111 != 0
                        && let Some(n) = e.file_name().to_str()
                    {
                        seen.insert(n.to_owned());
                    }
                }
            }
        }
        self.executables = seen.into_iter().collect();
        self.executables.sort();
        self.scanned = Instant::now();
    }
    /// Bundled specs apply only to installed commands (unless `installed_only` is off, as in tests).
    fn spec(&self, word: &str) -> Option<&Spec> {
        let installed = !self.installed_only
            || self
                .executables
                .binary_search_by(|e| e.as_str().cmp(word))
                .is_ok();
        self.specs
            .iter()
            .filter(|_| installed)
            .find(|s| s.commands[0].names.iter().any(|n| n == word))
    }
    /// `aws` with the services (and, once the words name one, the operations) of the installed
    /// CLI. While that data is still being read in the background the answer is `Pending`; the
    /// curated spec, whose root is `open`, is `Curated` and serves without CLI data.
    fn aws_spec(&self, previous: &[String], cwd: &Path) -> AwsSpec {
        if !self.filesystem || previous.first().map(String::as_str) != Some("aws") {
            return AwsSpec::Curated;
        }
        let Some(curated) = self.spec("aws") else {
            return AwsSpec::Curated;
        };
        let root = match self.aws.root(&self.runs, &self.path, curated) {
            aws_cli::Lookup::Absent => return AwsSpec::Curated,
            aws_cli::Lookup::Pending => return AwsSpec::Pending(None),
            aws_cli::Lookup::Ready(root) => root,
        };
        let mut command = SpecContext::new(Some(&root), previous, cwd).command;
        // The service is the ancestor just below the root.
        while let Some(parent) = root.commands[command].parent.filter(|&p| p != 0) {
            command = parent;
        }
        if root.commands[command].parent.is_none() {
            return AwsSpec::Ready(root);
        }
        let name = &root.commands[command].names[0];
        match self.aws.service(&self.runs, &self.path, curated, name) {
            aws_cli::Lookup::Ready(merged) => AwsSpec::Ready(merged),
            aws_cli::Lookup::Pending => AwsSpec::Pending(Some(root)),
            aws_cli::Lookup::Absent => AwsSpec::Ready(root),
        }
    }
    /// Names of bundled specs active for the current PATH.
    pub fn spec_status(&mut self) -> Vec<String> {
        self.scan_path();
        let active = |s: &Spec| self.spec(s.name()).is_some_and(|a| std::ptr::eq(a, s));
        self.specs
            .iter()
            .filter(|s| active(s))
            .map(|s| s.name().to_owned())
            .collect()
    }
    /// Called from a background thread when a tool-run value source has new results.
    /// Model runs announce themselves the same way.
    pub fn on_background_update(&mut self, f: impl Fn(&background::Key) + Send + Sync + 'static) {
        let f = std::sync::Arc::new(f);
        let g = f.clone();
        self.runs.on_update(move |k| f(k));
        if let Some(ai) = &mut self.ai {
            ai.on_update(move |k| g(k));
        }
    }
    /// Tool and model runs that the completions since the last call were waiting on.
    pub fn take_background_wanted(&self) -> Vec<background::Key> {
        let mut keys = self.runs.take_wanted();
        keys.extend(self.ai.iter().flat_map(ai::Ai::take_wanted));
        keys
    }
    pub fn background_running(&self, key: &background::Key) -> bool {
        match (&self.ai, key.1.first()) {
            (Some(ai), Some(prompt)) if key.0 == ai::KEY => ai.pending(prompt),
            _ => self.runs.is_running(key),
        }
    }
    pub fn background_busy(&self) -> bool {
        self.runs.busy() || self.ai.as_ref().is_some_and(ai::Ai::busy)
    }
    pub fn complete(&mut self, state: &State) -> Vec<Suggestion> {
        let effective_path = std::env::join_paths(std::env::split_paths(&state.path).map(|p| {
            if p.is_absolute() {
                p
            } else {
                state.cwd.join(p)
            }
        }))
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
        if self.path != effective_path {
            self.path = effective_path;
            self.scanned = Instant::now() - Duration::from_secs(60);
        }
        if let Some(request) = ai::request(&state.buffer) {
            return self.ai_complete(state, request);
        }
        let Some(c) = context(&state.buffer, state.cursor) else {
            return vec![];
        };
        if state.buffer.is_empty() || c.previous.first().is_some_and(|s| s.contains('=')) {
            return vec![];
        }
        let mut out = vec![];
        let suggestion = |label: &str, kind, description: String, insert: String| Suggestion {
            label: label.into(),
            kind,
            description,
            insert,
            start: c.start,
            end: c.end,
        };
        let command = c.previous.is_empty();
        if command && !c.prefix.contains('/') && !c.prefix.starts_with('~') {
            self.scan_path();
            for name in &self.executables {
                if name.starts_with(&c.prefix) {
                    out.push(suggestion(
                        name,
                        "executable",
                        "Executable from PATH".into(),
                        quote_word(name),
                    ));
                }
            }
            if "cd".starts_with(&c.prefix) {
                out.push(suggestion(
                    "cd",
                    "command",
                    "Change directory".into(),
                    "cd".into(),
                ));
            }
        }
        if !command {
            self.scan_path();
        }
        let aws = match self.aws_spec(&c.previous, &state.cwd) {
            AwsSpec::Ready(spec) | AwsSpec::Pending(Some(spec)) => Some(spec),
            _ => None,
        };
        let spec = aws
            .as_deref()
            .or_else(|| c.previous.first().and_then(|word| self.spec(word)));
        let ctx = SpecContext::new(spec, &c.previous, &state.cwd);
        let attached = if ctx.enabled && ctx.options && ctx.pending.is_none() {
            ctx.attached_value(&c.prefix)
        } else {
            None
        };
        let flag = ctx.pending.or(attached.map(|(_, _, flag)| flag));
        let arg = ctx.arg().filter(|_| ctx.enabled && flag.is_none());
        let value = match (flag, arg) {
            (Some(f), _) => ctx.takes_value(f).map(|v| &v.value),
            (None, Some(arg)) => Some(&arg.value.value),
            (None, None) => None,
        };
        let (refs, ref_kind) = match (value, ctx.cwd.as_deref()) {
            (Some(Value::Source(source)), Some(cwd)) if self.filesystem && ctx.enabled => {
                let env = Env {
                    cwd,
                    home: &self.home,
                    path: &self.path,
                    runs: &self.runs,
                };
                (source.complete(&env), source.suggestion_kind())
            }
            _ => (vec![], "ref"),
        };
        // Spec candidates stay borrowed until ranking picks the few that become Suggestions.
        let mut candidates: Vec<Candidate> = vec![];
        if let Some(spec) = spec.filter(|_| ctx.enabled) {
            let (option, prefix) = attached.map_or(("", c.prefix.as_str()), |(o, p, _)| (o, p));
            for r in refs.iter().filter(|r| r.name.starts_with(prefix)) {
                candidates.push(Candidate {
                    label: &r.name,
                    kind: ref_kind,
                    description: &r.description,
                    names: &[],
                    option,
                    inherited: false,
                });
            }
            if let Some(Value::Choices(choices)) = value {
                let description = match flag {
                    Some(f) => &spec.flags[f].description,
                    None => &arg.unwrap().description,
                };
                for choice in choices.iter().filter(|choice| choice.starts_with(prefix)) {
                    candidates.push(Candidate {
                        label: choice,
                        kind: "value",
                        description,
                        names: &[],
                        option,
                        inherited: false,
                    });
                }
            }
            if flag.is_none() && ctx.options {
                if ctx.positionals == 0 {
                    for &child in &spec.commands[ctx.command].children {
                        let child = &spec.commands[child];
                        for name in child.names.iter().filter(|n| n.starts_with(&c.prefix)) {
                            candidates.push(Candidate {
                                label: name,
                                kind: "command",
                                description: &child.description,
                                names: &child.names,
                                option: "",
                                inherited: false,
                            });
                        }
                    }
                }
                for &f in &ctx.visible {
                    if ctx.hidden(f) {
                        continue;
                    }
                    let entry = &spec.flags[f];
                    for name in &entry.names {
                        // Skip spellings shadowed by a closer flag.
                        if name.starts_with(&c.prefix) && spec.flag(ctx.command, name) == Some(f) {
                            candidates.push(Candidate {
                                label: name,
                                kind: "flag",
                                description: &entry.description,
                                names: &entry.names,
                                option: "",
                                inherited: entry.command != ctx.command,
                            });
                        }
                    }
                }
            }
        }
        // Inherited flags (an ancestor's persistent ones, like all of aws's global options) come last,
        // so the cut below drops them before the command's own flags.
        candidates.sort_by_cached_key(|x| {
            (
                x.label != c.prefix,
                tier(x.kind) + 10 * u8::from(x.inherited),
                x.label.to_lowercase(),
            )
        });
        let inherited: HashSet<&str> = candidates
            .iter()
            .filter(|x| x.inherited)
            .map(|x| x.label)
            .collect();
        // Slack for later deduplication against filesystem entries with the same insertion.
        for x in candidates.iter().take(2 * MAX_RESULTS) {
            let insert = if matches!(x.kind, "value" | "ref") {
                format!("{}{}", x.option, quote_word(x.label))
            } else {
                x.label.into()
            };
            out.push(suggestion(
                x.label,
                x.kind,
                described(x.description, x.names),
                insert,
            ));
        }
        // Flag text values have no source; text positionals (e.g. <branch>) often name paths too.
        let files = value.is_none_or(|v| v.offers_paths(flag.is_some()));
        if files
            && self.filesystem
            && (!command || c.prefix.contains('/') || c.prefix.starts_with('~'))
            && let Some(cwd) = ctx.cwd.as_deref()
        {
            let only_dirs =
                value == Some(&Value::Directory) || c.previous.first().is_some_and(|s| s == "cd");
            if let Some((option, prefix, _)) = attached {
                let file_context = Context {
                    prefix: prefix.into(),
                    start: c.start,
                    end: c.end,
                    previous: c.previous.clone(),
                    // Tildes inside an option token are not ordinary shell tilde expansions.
                    expand_tilde: false,
                };
                let before = out.len();
                self.files(&file_context, cwd, only_dirs, &mut out);
                for suggestion in &mut out[before..] {
                    suggestion.insert = format!("{option}{}", suggestion.insert);
                }
            } else {
                self.files(&c, cwd, only_dirs, &mut out);
            }
        }
        out.sort_by_cached_key(|x| {
            let rank = tier(x.kind) + 10 * u8::from(inherited.contains(x.insert.as_str()));
            (
                if x.label == c.prefix { 0 } else { rank },
                x.label.to_lowercase(),
            )
        });
        out.dedup_by(|a, b| a.insert == b.insert);
        out.truncate(MAX_RESULTS);
        out
    }
    /// Model suggestions for `# request`, replacing the whole buffer. Until they are ready, or if
    /// none is usable, one `status` row says why; accepting it only closes the popup.
    fn ai_complete(&mut self, state: &State, request: &str) -> Vec<Suggestion> {
        if self.ai.is_none() {
            return vec![];
        }
        let tools = self.spec_status();
        let directory = state.cwd.file_name().unwrap_or_default().to_string_lossy();
        let prompt = ai::prompt(request, &directory, &tools);
        let end = state.buffer.chars().count();
        let status = |label: &str, description: &str| {
            vec![Suggestion {
                label: label.into(),
                kind: STATUS,
                description: description.into(),
                insert: String::new(),
                start: 0,
                end,
            }]
        };
        let commands = match self.ai.as_ref().and_then(|ai| ai.get(&prompt)) {
            None => return status("Asking the on-device model…", "AI · Takes a few seconds"),
            Some(None) => {
                return status(
                    "The on-device model didn't answer",
                    "AI · Check Apple Intelligence and `fm available`",
                );
            }
            Some(Some(commands)) => commands,
        };
        let mut out: Vec<Suggestion> = vec![];
        for c in commands {
            if out.iter().any(|s| s.insert == c.command)
                || !self.valid_command(&c.command, &state.cwd)
            {
                continue;
            }
            let explanation: String = c
                .explanation
                .chars()
                .filter(|&c| !spec::deceptive(c))
                .collect();
            out.push(Suggestion {
                label: c.command.clone(),
                kind: "ai",
                description: format!("AI · {explanation}"),
                // A whole command line, inserted as-is: quoting it would make it one word.
                insert: c.command,
                start: 0,
                end,
            });
        }
        if out.is_empty() {
            return status("No usable command", "AI · Try rewording the request");
        }
        out
    }
    /// Lexable, not denied, every command on PATH and, where a spec exists, only words it knows.
    fn valid_command(&self, line: &str, cwd: &Path) -> bool {
        let Some(segments) = ai::segments(line) else {
            return false;
        };
        !segments.is_empty()
            && !ai::denied(&segments, line)
            && segments.iter().all(|words| {
                let first = &words[0];
                !first.contains('=') && self.executables.binary_search(first).is_ok() && {
                    let aws = match self.aws_spec(words, cwd) {
                        // Unverifiable until the CLI's data is read; a retry then decides.
                        AwsSpec::Pending(_) => return false,
                        AwsSpec::Ready(spec) => Some(spec),
                        AwsSpec::Curated => None,
                    };
                    aws.as_deref()
                        .or_else(|| self.spec(first))
                        .is_none_or(|spec| !SpecContext::new(Some(spec), words, cwd).unknown)
                }
            })
    }
    fn files(&self, c: &Context, cwd: &Path, only_dirs: bool, out: &mut Vec<Suggestion>) {
        let (parent, prefix) = match c.prefix.rsplit_once('/') {
            Some((p, n)) => (format!("{p}/"), n),
            None => (String::new(), c.prefix.as_str()),
        };
        let expanded = if c.expand_tilde && parent == "~/" {
            self.home.clone()
        } else if c.expand_tilde && parent.starts_with("~/") {
            let p = parent.strip_prefix("~/").unwrap();
            self.home.join(p)
        } else if c.expand_tilde && parent.starts_with('~') {
            return;
        } else {
            cwd.join(&parent)
        };
        if let Ok(entries) = fs::read_dir(expanded) {
            for e in entries.take(20000).flatten() {
                let Some(name) = e.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !name.starts_with(prefix) || (name.starts_with('.') && !prefix.starts_with('.'))
                {
                    continue;
                }
                let dir = e.path().is_dir();
                if only_dirs && !dir {
                    continue;
                }
                let suffix = if dir { "/" } else { "" };
                let value = format!("{parent}{name}{suffix}");
                // Quote expanded home path: a quoted tilde would otherwise become a literal tilde.
                let insert = if c.expand_tilde && value.starts_with("~/") {
                    let rest = value.strip_prefix("~/").unwrap();
                    quote_word(&self.home.join(rest).to_string_lossy())
                } else {
                    quote_word(&value)
                };
                out.push(Suggestion {
                    label: format!("{name}{suffix}"),
                    kind: if dir { "directory" } else { "file" },
                    description: if dir { "Directory" } else { "File" }.into(),
                    insert,
                    start: c.start,
                    end: c.end,
                });
            }
        }
    }
}
/// Run a spec's embedded `#test` lines against only that spec; filesystem and PATH are never read.
pub fn spec_test_failures(spec: &Spec, label: &str) -> Vec<String> {
    let cwd = PathBuf::from("/nonexistent-riga-spec-test");
    let mut engine = Engine::new(String::new(), cwd.clone());
    engine.filesystem = false;
    engine.specs = vec![spec.clone()];
    let mut failures = vec![];
    for test in &spec.tests {
        let labels: Vec<String> = engine
            .complete(&State {
                session: "spec-test".into(),
                pane: "spec-test".into(),
                revision: 1,
                cursor: test.buffer.chars().count(),
                cwd: cwd.clone(),
                buffer: test.buffer.clone(),
                path: String::new(),
            })
            .into_iter()
            .map(|s| s.label)
            .collect();
        for (shown, item) in &test.expect {
            if labels.contains(item) != *shown {
                let expected = if *shown { "shown" } else { "absent" };
                failures.push(spec::printable(&format!(
                    "{label}:{}: expected `{item}` to be {expected}",
                    test.line
                )));
            }
        }
    }
    failures
}
#[cfg(test)]
mod tests {
    use super::*;
    fn complete_in(buffer: &str, cwd: &Path) -> Vec<Suggestion> {
        Engine::new(String::new(), cwd.into()).complete(&State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor: buffer.chars().count(),
            cwd: cwd.into(),
            buffer: buffer.into(),
            path: String::new(),
        })
    }
    fn flags(buffer: &str) -> Vec<String> {
        complete_in(buffer, Path::new("/nonexistent-riga-test-directory"))
            .into_iter()
            .filter(|suggestion| suggestion.kind == "flag")
            .map(|suggestion| suggestion.label)
            .collect()
    }
    #[test]
    fn catalog_schema_and_aliases_are_unambiguous() {
        for (label, text) in BUNDLED_SPECS {
            let spec = spec::parse(label, text).unwrap_or_else(|e| panic!("{e:?}"));
            for command in &spec.commands {
                assert!(
                    !command.description.is_empty(),
                    "{label} {:?}",
                    command.names
                );
            }
            for flag in &spec.flags {
                assert!(!flag.description.is_empty(), "{label} {:?}", flag.names);
            }
            assert!(spec.tests.len() >= 10, "{label}");
            assert_eq!(spec_test_failures(&spec, label), Vec::<String>::new());
        }
        assert_eq!(Engine::new(String::new(), "/".into()).specs.len(), 18);
    }
    const TEST_SPEC: &str = r#"
command t "Test tool"
  flag --verbose|-v persistent repeat "Verbose"
  flag --root "Root only"
  flag --into <directory> chdir "Change directory"
  command sub "Leaf with args"
    flag --a|-a "A"
    flag --b|-b "B"
    flag --mode|-m {fast,slow} "Mode"
    flag --color [{auto,never}] "Color"
    flag --x excludes=--y "X"
    flag --y "Y"
    flag --dep requires=--a,--b "Needs a and b"
    flag -v "Shadows the persistent short spelling"
    arg {red,green} "Color arg"
    arg <directory> "Dir arg"
    arg <file> repeat "Files"
  command pack nobundle "No bundling"
    flag -a "A"
    flag -b "B"
    flag -o <file> "Output"
  command run "Stops"
    arg <image> stop "Image"
    arg [<rest>] repeat "Rest"
  command parent "Children and args"
    command child "Child"
    arg <thing> "Thing"
"#;
    fn spec_engine(root: &Path) -> Engine {
        let mut engine = Engine::new(String::new(), root.into());
        engine.specs = vec![spec::parse("t.riga", TEST_SPEC).unwrap()];
        engine
    }
    fn labels(engine: &mut Engine, buffer: &str, cwd: &Path) -> Vec<String> {
        let state = State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor: buffer.chars().count(),
            cwd: cwd.into(),
            buffer: buffer.into(),
            path: String::new(),
        };
        engine
            .complete(&state)
            .into_iter()
            .map(|s| s.label)
            .collect()
    }
    fn spec_labels(buffer: &str) -> Vec<String> {
        let nowhere = Path::new("/nonexistent-riga-test-directory");
        labels(&mut spec_engine(nowhere), buffer, nowhere)
    }
    fn has(buffer: &str, shown: &[&str], absent: &[&str]) {
        let got = spec_labels(buffer);
        for s in shown {
            assert!(
                got.contains(&s.to_string()),
                "{buffer}: missing {s} in {got:?}"
            );
        }
        for s in absent {
            assert!(
                !got.contains(&s.to_string()),
                "{buffer}: unexpected {s} in {got:?}"
            );
        }
    }
    #[test]
    fn spec_choices_excludes_requires_and_scopes() {
        assert_eq!(spec_labels("t sub -m "), vec!["fast", "slow"]);
        assert_eq!(spec_labels("t sub --mode f"), vec!["fast"]);
        let attached = complete_in("t sub --mode=s", Path::new("/nonexistent"));
        assert!(attached.is_empty(), "t is not a bundled spec");
        let mut engine = spec_engine(Path::new("/nonexistent"));
        let state = State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor: 14,
            cwd: "/nonexistent".into(),
            buffer: "t sub --mode=s".into(),
            path: String::new(),
        };
        let suggestion = &engine.complete(&state)[0];
        assert_eq!(
            (suggestion.kind, suggestion.insert.as_str()),
            ("value", "--mode=slow")
        );
        assert_eq!(spec_labels("t sub -mf"), vec!["fast"]);
        has("t sub --x --", &["--a"], &["--y", "--x"]);
        has("t sub --y --", &["--a"], &["--x", "--y"]);
        has("t sub --a --", &["--b"], &["--dep"]);
        has("t sub --a -b --", &["--dep"], &[]);
        has("t sub --", &["--verbose"], &["--root", "--into"]);
        has("t sub -", &["-v"], &[]);
        has("t sub -v -v --", &["--verbose"], &[]);
        has("t --root sub --", &["--a"], &[]);
        assert!(spec_labels("t sub --root --").is_empty());
        has("t --", &["--root", "--verbose"], &[]);
    }
    #[test]
    fn spec_bundles_positionals_and_stop() {
        has("t sub -ab --", &["--x"], &["--a", "--b"]);
        has("t pack -a -", &["-b"], &["-a"]);
        assert!(spec_labels("t pack -ab -").is_empty());
        has("t pack -oout.txt -", &["-a", "-b"], &["-o"]);
        has("t sub ", &["red", "green", "--a"], &[]);
        has("t sub r", &["red"], &["green"]);
        has("t sub red ", &["--a"], &["red"]);
        has("t sub -- ", &["red"], &["--a"]);
        has("t sub -- --", &[], &["--a", "--x"]);
        has("t sub --color --", &["--a", "--mode"], &[]);
        has("t sub --color=", &["auto", "never"], &["--a"]);
        has("t sub --color red ", &[], &["red"]);
        has("t ", &["sub", "pack", "--root"], &[]);
        assert!(spec_labels("t unknown --").is_empty());
        has("t parent chi", &["child"], &[]);
        has("t parent thing chi", &[], &["child"]);
        has("t parent child --", &["--verbose"], &[]);
        has("t run -", &["--verbose"], &[]);
        assert!(spec_labels("t run image --").is_empty());
        let root = std::env::temp_dir().join(format!("riga-spec-args-{}", std::process::id()));
        fs::create_dir_all(root.join("dir")).unwrap();
        fs::write(root.join("file"), "").unwrap();
        let mut engine = spec_engine(&root);
        assert_eq!(labels(&mut engine, "t sub -- red ", &root), vec!["dir/"]);
        assert_eq!(
            labels(&mut engine, "t sub red dir/ ", &root),
            vec![
                "--a",
                "--b",
                "--color",
                "--mode",
                "--x",
                "--y",
                "-a",
                "-b",
                "-m",
                "-v",
                "dir/",
                "file",
                // Inherited (persistent) flags rank after everything else.
                "--verbose"
            ]
        );
        assert_eq!(
            labels(&mut engine, "t sub -- red dir/ x ", &root),
            vec!["dir/", "file"]
        );
        assert_eq!(
            labels(&mut engine, "t run image ", &root),
            vec!["dir/", "file"]
        );
        assert_eq!(labels(&mut engine, "t sub red d", &root), vec!["dir/"]);
        assert!(labels(&mut engine, "t --into dir sub red d", &root).is_empty());
        let mut engine = Engine::new(String::new(), root.clone());
        assert!(labels(&mut engine, "git checkout ", &root).contains(&"file".into()));
        assert_eq!(labels(&mut engine, "docker build d", &root), vec!["dir/"]);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ai_suggestions_are_validated_and_replace_the_whole_buffer() {
        let root = std::env::temp_dir().join(format!("riga-ai-{}", std::process::id()));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for tool in ["git", "lsof", "grep", "sudo"] {
            fs::write(bin.join(tool), "").unwrap();
            fs::set_permissions(bin.join(tool), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let reply = [
            ("lsof -iTCP -sTCP:LISTEN | grep x", "Listening ports"),
            ("git status --short", r"Short status \u202e"),
            ("git status --bogus", "Unknown flag"),
            ("git frobnicate", "Unknown subcommand"),
            ("nosuchtool -x", "Not on PATH"),
            (r"ls\nrm x", "Newline"),
            ("sudo lsof", "Denied"),
            ("lsof -i > ports.txt", "Redirect"),
        ]
        .map(|(c, e)| format!(r#"{{"command":"{c}","explanation":"{e}"}}"#))
        .join(",");
        let prompts = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let p = prompts.clone();
        let mut engine = Engine::new(String::new(), root.clone());
        engine.ai = Some(ai::Ai::with_backend(move |prompt| {
            p.lock().unwrap().push(prompt.to_owned());
            Some(format!(r#"{{"commands":[{reply}]}}"#))
        }));
        let (tx, rx) = std::sync::mpsc::channel();
        engine.on_background_update(move |k| tx.send(k.clone()).unwrap());
        let buffer = "# list ports é🌍 now";
        let state = State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor: 3,
            cwd: root.join("proj"),
            buffer: buffer.into(),
            path: bin.to_string_lossy().into_owned(),
        };
        let waiting = engine.complete(&state);
        assert_eq!(
            (waiting.len(), waiting[0].kind),
            (1, STATUS),
            "progress while the model runs"
        );
        let key = engine.take_background_wanted().pop().unwrap();
        assert!(engine.background_running(&key));
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), key);
        assert!(!engine.background_running(&key));
        let prompt = &prompts.lock().unwrap()[0];
        assert!(prompt.contains("list ports é🌍 now") && prompt.contains("directory name: proj"));
        assert!(prompt.contains("Installed tools: git"), "{prompt}");
        let items = engine.complete(&state);
        let got: Vec<_> = items
            .iter()
            .map(|s| (s.insert.as_str(), s.description.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("lsof -iTCP -sTCP:LISTEN | grep x", "AI · Listening ports"),
                ("git status --short", "AI · Short status "),
            ]
        );
        assert!(
            items
                .iter()
                .all(|s| (s.start, s.end) == (0, 19) && s.kind == "ai")
        );
        // Failures say so instead of leaving the popup empty.
        for (reply, label) in [
            (None, "The on-device model didn't answer"),
            (
                Some(r#"{"commands":[{"command":"sudo lsof","explanation":"x"}]}"#),
                "No usable command",
            ),
        ] {
            engine.ai = Some(ai::Ai::with_backend(move |_| reply.map(str::to_owned)));
            let (tx, rx) = std::sync::mpsc::channel();
            engine.on_background_update(move |_| tx.send(()).unwrap());
            assert_eq!(engine.complete(&state)[0].kind, STATUS);
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let items = engine.complete(&state);
            assert_eq!(
                (items.len(), items[0].label.as_str(), items[0].kind),
                (1, label, STATUS)
            );
            assert!(items[0].insert.is_empty());
        }
        // Turned off (`RIGA_AI=0`): `#` lines stay silent.
        engine.ai = None;
        assert!(engine.complete(&state).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn bundled_specs_need_the_command_on_path() {
        let root = std::env::temp_dir().join(format!("riga-installed-{}", std::process::id()));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let mut engine = Engine::new(String::new(), root.clone());
        engine.installed_only = true;
        let mut flags = |buffer: &str| -> Vec<String> {
            let state = State {
                session: "s".into(),
                pane: "p".into(),
                revision: 1,
                cursor: buffer.chars().count(),
                cwd: root.clone(),
                buffer: buffer.into(),
                path: bin.to_string_lossy().into_owned(),
            };
            engine.scanned = Instant::now() - Duration::from_secs(60);
            engine
                .complete(&state)
                .into_iter()
                .filter(|s| s.kind == "flag")
                .map(|s| s.label)
                .collect()
        };
        assert!(flags("git --").is_empty());
        fs::write(bin.join("git"), "").unwrap();
        fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!flags("git --").is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn spec_parse_errors_report_lines() {
        // Echoed tokens never carry raw escape sequences to the terminal.
        let echoed = spec::parse("x.riga", "command t\n  flag --a\u{1b}]0;title\u{7}")
            .unwrap_err()
            .join("\n");
        assert_eq!(echoed, "x.riga:2: invalid name `--a\\u{1b}]0;title\\u{7}`");
        let error = |text: &str| spec::parse("x.riga", text).unwrap_err().join("\n");
        for (text, message) in [
            (
                "command t\n\tflag --a",
                "x.riga:2: tabs are not allowed in indentation",
            ),
            (
                "command t\n  flag --a\n    flag --b",
                "x.riga:3: only commands may have children",
            ),
            (
                "command t\n    flag --a",
                "x.riga:2: indentation may grow by only one level",
            ),
            (
                "command t\n   flag --a",
                "x.riga:2: indentation must be a multiple of two spaces",
            ),
            (
                "command t\ncommand u",
                "x.riga:2: the top level must be exactly one `command`",
            ),
            ("# empty", "x.riga:1: missing top-level `command`"),
            (
                "command t\n  flag a",
                "x.riga:2: flag names must start with `-`, got `a`",
            ),
            (
                "command t\n  command -a",
                "x.riga:2: command names must not start with `-`",
            ),
            (
                "command t\n  flag --a\n  flag -b|--a",
                "x.riga:3: duplicate flag `--a`",
            ),
            (
                "command t\n  command a\n  command b|a",
                "x.riga:3: duplicate command `a`",
            ),
            (
                "command t\n  flag --a fast",
                "x.riga:2: unknown flag attribute `fast`",
            ),
            (
                "command t\n  option --a",
                "x.riga:2: unknown keyword `option`",
            ),
            (
                "command t\n  flag --d <file> chdir",
                "x.riga:2: `chdir` requires a <directory> value",
            ),
            (
                "command t\n  arg <a> repeat\n  arg <b>",
                "x.riga:3: a `repeat` arg must be the last arg",
            ),
            (
                "command t\n  flag --c {}",
                "x.riga:2: choices must be non-empty",
            ),
            (
                "command t\n  flag --c [[<x>]]",
                "x.riga:2: optional values cannot be nested",
            ),
            ("command t \"open", "x.riga:1: unterminated quoted string"),
            // A rejected flag line must not leave dangling requires/excludes references.
            (
                "command t\n  flag --d requires=--zz,",
                "x.riga:2: `requires` needs flag names, got ``",
            ),
            (
                "command t\n  flag --d requires=--zz bogus",
                "x.riga:2: unknown flag attribute `bogus`",
            ),
            (
                "command t\n  flag --d excludes=--e chdir",
                "x.riga:2: `chdir` requires a <directory>",
            ),
            (
                "command t\n  flag --e\n  flag --e excludes=--e",
                "x.riga:3: duplicate flag `--e`",
            ),
            (
                "command t\n  flag --a|--a",
                "x.riga:2: duplicate name `--a`",
            ),
            ("command s|s", "x.riga:1: duplicate name `s`"),
            (
                "command t\n  flag --a excludes=--a",
                "x.riga:2: flag cannot reference itself",
            ),
            (
                "command t \"d\" nobundle",
                "x.riga:1: the description must be the last token",
            ),
            (
                "command t\n  flag --a requires=--zz",
                "x.riga:2: `--zz` is not a flag visible here",
            ),
            (
                "command t\n  command s\n    flag --a requires=--r\n  flag --r",
                "x.riga:3: `--r` is not a flag visible here",
            ),
            (
                "command t\n#test git => +x",
                "x.riga:2: lines starting with #test must read",
            ),
            (
                "command t\n#testing later",
                "x.riga:2: lines starting with #test must read",
            ),
            (
                "command t\n#test\"t\" => +x",
                "x.riga:2: lines starting with #test must read",
            ),
            (
                "command t\n#test\t\"t\" => +x",
                "x.riga:2: lines starting with #test must read",
            ),
            (
                "command t\n  \"orphan\"",
                "x.riga:2: a description must follow a statement",
            ),
            (
                "command t nobundle nobundle",
                "x.riga:1: duplicate attribute `nobundle`",
            ),
            (
                "command t\n  flag --a repeat repeat",
                "x.riga:2: duplicate attribute `repeat`",
            ),
            (
                "command t\n  flag --x\n  flag --a requires=--x requires=--x",
                "x.riga:3: duplicate attribute `requires`",
            ),
            (
                "command t\n  flag --y\n  flag --a requires=--y,--y",
                "x.riga:3: duplicate reference `--y`",
            ),
            (
                "command t\n  arg <a> stop stop",
                "x.riga:2: duplicate attribute `stop`",
            ),
            ("command t\n  flag --a!b", "x.riga:2: invalid name `--a!b`"),
            (
                "command t \"\u{202e}gnirts\"",
                "x.riga:1: descriptions must not contain control",
            ),
            (
                "command t \"a\u{2028}b\"",
                "x.riga:1: descriptions must not contain control",
            ),
            (
                "command t\n  flag --c {a,b\u{1b}}",
                "x.riga:2: choices must be non-empty words",
            ),
            (
                "command t\n  flag --c [[<x>]]",
                "x.riga:2: optional values cannot be nested",
            ),
            (
                "command t\n#test \"t \" => x",
                "x.riga:2: expectation `x` must be +label or -label",
            ),
        ] {
            assert!(error(text).contains(message), "{text:?}: {}", error(text));
        }
        assert_eq!(error("command t\n  flag a\n  flag b").lines().count(), 2);
        let sorted = error("command t\n  flag --a requires=--zz\n  flag b");
        assert!(
            sorted.starts_with("x.riga:2:") && sorted.contains("\nx.riga:3:"),
            "{sorted}"
        );
        // Malformed input of every kind reports errors instead of panicking.
        for text in [
            "",
            "\t",
            " ",
            "  command t",
            "command",
            "flag --a",
            "arg <x>",
            "command t\n  arg",
            "command t\n  arg <",
            "command t\n  arg [",
            "command t\n  arg []",
            "command t\n  flag --a <>",
            "command t\n  flag --a {,}",
            "command t\n  flag --a requires=",
            "command t\n  flag --a excludes=,",
            "command t\n  flag --a requires=--b\n  flag --b requires=--a,--c",
            "command t\n  flag - \"x\"",
            "command t\n  flag -- ",
            "command t \"a\\x\"",
            "command t\n#test",
            "command t\n#test \"",
            "command t\n#test \"x\"",
            "command t\n#test \"x\" =>",
            "command t\n#test \"x\" => +",
            "command t\n#test \"x\" => \u{e9}x",
            "command é\n  flag -é <ü> \"ö\"",
            "command t\n  command u\n      flag --a",
            "command t\n  command u|-v",
            "command t\n  flag --a|b",
            "command t x\"y\"",
        ] {
            let _ = spec::parse("x.riga", text);
        }
        // Deep brackets must not recurse: this used to overflow the worker's stack.
        let deep = format!(
            "command t\n  arg {}<x>{}",
            "[".repeat(30000),
            "]".repeat(30000)
        );
        assert!(error(&deep).contains("x.riga:2: optional values cannot be nested"));
        assert_eq!(
            error("\u{feff}command t\n  flag a"),
            "x.riga:2: flag names must start with `-`, got `a`"
        );
        assert_eq!(
            error("command -t\n  flag --a"),
            "x.riga:1: command names must not start with `-`, got `-t`"
        );
        let spec = spec::parse("x.riga", "command t # comment\n  flag --c {a#b} #note\n").unwrap();
        assert_eq!(
            spec.flags[0].value.as_ref().unwrap().value,
            spec::Value::Choices(vec!["a#b".into()])
        );
        let spec = spec::parse("x.riga", "command t\n  command s\n    flag --a requires=--r\n  flag --r persistent\n#test \"t s \" => +--zz -x").unwrap();
        assert_eq!(
            spec.tests[0].expect,
            vec![(true, "--zz".into()), (false, "x".into())]
        );
        assert_eq!(
            spec_test_failures(&spec, "x.riga"),
            vec!["x.riga:5: expected `--zz` to be shown"]
        );
    }
    #[test]
    fn pathological_specs_stay_bounded() {
        // Child lookup is a hash map checked only in the first slot: words × children stays linear.
        let children: String = (0..20_000).map(|i| format!("  command c{i}\n")).collect();
        let mut engine = Engine::new(String::new(), "/nonexistent".into());
        engine.filesystem = false;
        engine.specs = vec![
            spec::parse(
                "kids.riga",
                &format!("command t\n{children}  arg <x> repeat\n"),
            )
            .unwrap(),
        ];
        let nowhere = Path::new("/nonexistent");
        let started = Instant::now();
        let words = format!("t {}c", "x ".repeat(20_000));
        assert!(labels(&mut engine, &words, nowhere).is_empty());
        assert_eq!(labels(&mut engine, "t c19999", nowhere), vec!["c19999"]);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(
            error_of(&format!("command {}", "n".repeat(129)))
                .contains("x.riga:1: names are limited to 128 characters")
        );
        // Counts are uncapped (loading is hash-indexed and linear); displayed text is bounded.
        let names = |n: usize| {
            (0..n)
                .map(|i| format!("--a{i}"))
                .collect::<Vec<_>>()
                .join("|")
        };
        assert!(
            error_of(&format!("command t\n  flag {}", names(65)))
                .contains("x.riga:2: more than 64 names")
        );
        let long = "d".repeat(201);
        assert!(
            error_of(&format!("command t \"{long}\""))
                .contains("x.riga:1: descriptions are limited to 200")
        );
        let flags: String = (0..3000).map(|i| format!("  flag --f{i}\n")).collect();
        let spec = spec::parse(
            "x.riga",
            &format!("command t\n  flag {}\n{flags}", names(64)),
        )
        .unwrap();
        assert_eq!((spec.flags.len(), spec.flags[0].names.len()), (3001, 64));
        // Completion ranks borrowed candidates and builds at most 128 suggestions.
        let desc = "d".repeat(200);
        let many: String = (0..500)
            .map(|f| {
                let spellings: Vec<String> = (0..64).map(|n| format!("--f{f}n{n}")).collect();
                format!("  flag {} \"{desc}\"\n", spellings.join("|"))
            })
            .collect();
        let choices: Vec<String> = (0..40_000).map(|i| format!("c{i}")).collect();
        let text = format!(
            "command t\n{many}  command s\n    arg {{{}}} \"{desc}\"\n",
            choices.join(",")
        );
        let started = Instant::now();
        let mut engine = Engine::new(String::new(), "/nonexistent".into());
        engine.filesystem = false;
        engine.specs = vec![spec::parse("wide.riga", &text).unwrap()];
        let nowhere = Path::new("/nonexistent");
        let flags = labels(&mut engine, "t -", nowhere);
        assert_eq!((flags.len(), flags[0].as_str()), (MAX_RESULTS, "--f0n0"));
        let values = labels(&mut engine, "t s c1", nowhere);
        assert_eq!((values.len(), values[0].as_str()), (MAX_RESULTS, "c1"));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        // Only the earliest errors are kept; the rest are counted.
        let many: String = (0..2000).map(|i| format!("  flag f{i}\n")).collect();
        let errors = spec::parse("x.riga", &format!("command t\n{many}")).unwrap_err();
        assert_eq!(errors.len(), 101);
        assert!(errors[0].starts_with("x.riga:2: flag names"));
        assert!(errors[100].ends_with("x.riga: 1900 more errors not shown"));
        // Children of a rejected command are skipped instead of cascading.
        let rejected =
            "command t\n  command -x\n    flag --a\n    command y\n      flag --b\n  flag --c";
        assert_eq!(
            error_of(rejected),
            "x.riga:2: command names must not start with `-`, got `-x`"
        );
        // Depth is the one structural cap: it bounds per-word ancestor walks.
        let mut text = String::from("command t\n");
        for depth in 1..=33 {
            let indent = "  ".repeat(depth);
            for f in 0..16 {
                let names: Vec<String> = (0..4).map(|n| format!("--d{depth}f{f}n{n}")).collect();
                let refs = if f > 0 {
                    format!(" excludes=--d{depth}f0n0")
                } else {
                    String::new()
                };
                text += &format!("{indent}flag {} persistent{refs} \"F\"\n", names.join("|"));
            }
            text += &format!("{indent}command c{depth}\n");
        }
        assert!(error_of(&text).contains("nesting deeper than 32 levels"));
        let text: String = text
            .lines()
            .filter(|l| !l.starts_with(&"  ".repeat(33)))
            .map(|l| format!("{l}\n"))
            .collect();
        let started = Instant::now();
        let mut engine = Engine::new(String::new(), "/nonexistent".into());
        engine.filesystem = false;
        engine.specs = vec![spec::parse("deep.riga", &text).unwrap()];
        let chain: Vec<String> = (1..=32).map(|d| format!("c{d}")).collect();
        let nowhere = Path::new("/nonexistent");
        let buffer = format!("t {} --d32f0n0 --d32f1n", chain.join(" "));
        assert!(labels(&mut engine, &buffer, nowhere).is_empty());
        let buffer = format!("t {} --d1f0n0 --d32f1n", chain.join(" "));
        assert_eq!(labels(&mut engine, &buffer, nowhere).len(), 4);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
    #[test]
    fn short_value_flags_do_not_swallow_single_dash_long_flags() {
        let spec = "command j\n  flag -c <class> \"Class\"\n  flag -cp|-classpath <path> \"Path\"\n  flag -v \"V\"";
        let mut engine = Engine::new(String::new(), "/nonexistent".into());
        engine.specs = vec![spec::parse("j.riga", spec).unwrap()];
        let nowhere = Path::new("/nonexistent");
        assert_eq!(labels(&mut engine, "j -cp", nowhere), vec!["-cp"]);
        assert_eq!(labels(&mut engine, "j -cl", nowhere), vec!["-classpath"]);
        assert_eq!(
            labels(&mut engine, "j -c", nowhere),
            vec!["-c", "-classpath", "-cp"]
        );
        assert!(labels(&mut engine, "j -cx", nowhere).is_empty());
        assert!(labels(&mut engine, "j -cp ", nowhere).is_empty());
        assert_eq!(
            labels(&mut engine, "j -cfoo -", nowhere),
            vec!["-classpath", "-cp", "-v"]
        );
    }
    fn error_of(text: &str) -> String {
        spec::parse("x.riga", text).unwrap_err().join("\n")
    }
    const OPEN_SPEC: &str = r#"
command o open "Open tool"
  flag --verbose|-v persistent "Verbose"
  flag --root "Root only"
  command known "Known"
    flag --k "K"
"#;
    #[test]
    fn open_commands_keep_completing_after_unknown_words() {
        let nowhere = Path::new("/nonexistent-riga-test-directory");
        let spec = spec::parse("o.riga", OPEN_SPEC).unwrap();
        let mut engine = Engine::new(String::new(), nowhere.into());
        engine.specs = vec![spec.clone()];
        // Long flags and subcommands only.
        let mut at = |buffer: &str| {
            let mut got = labels(&mut engine, buffer, nowhere);
            got.retain(|l| !l.starts_with('-') || l.starts_with("--"));
            got
        };
        assert_eq!(at("o "), ["--root", "--verbose", "known"]);
        // An unknown word: only persistent flags stay, and the subcommands are gone.
        assert_eq!(at("o zzz "), ["--verbose"]);
        assert_eq!(at("o zzz yyy --v"), ["--verbose"]);
        assert_eq!(at("o zzz --verbose "), Vec::<String>::new());
        assert_eq!(at("o known "), ["--k", "--verbose"]);
        // An unknown option stays enabled and takes a following plain word as its value.
        assert_eq!(at("o --x "), ["--root", "--verbose", "known"]);
        assert_eq!(at("o --x --"), ["--root", "--verbose"]);
        assert_eq!(at("o --x=1 "), ["--root", "--verbose", "known"]);
        assert_eq!(at("o --x v known "), ["--k", "--verbose"]);
        // A scope that is not open still stops at unknown words and options.
        assert!(at("o known --nope --").is_empty());
        for words in [
            &["o", "zzz", "--bad", "v", "yyy"][..],
            &["o", "--bad", "known", "--k"],
        ] {
            let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
            let ctx = SpecContext::new(Some(&spec), &words, nowhere);
            assert!(!ctx.unknown && ctx.enabled, "{words:?}");
        }
        assert!(error_of("command t open open").contains("duplicate attribute `open`"));
        assert!(
            error_of("command t\n  command u\n  flag --a open").contains("unknown flag attribute")
        );
    }
    fn aws_state(buffer: &str, path: &str) -> State {
        State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor: buffer.chars().count(),
            cwd: "/nonexistent-riga-test-directory".into(),
            buffer: buffer.into(),
            path: path.into(),
        }
    }
    /// Complete `buffer`, letting background reads of the CLI data finish in between.
    fn aws_labels(engine: &mut Engine, buffer: &str, path: &str) -> Vec<String> {
        let mut got = vec![];
        for _ in 0..3 {
            got = engine
                .complete(&aws_state(buffer, path))
                .into_iter()
                .map(|s| s.label)
                .collect();
            let start = Instant::now();
            while engine.background_busy() {
                assert!(
                    start.elapsed() < Duration::from_secs(30),
                    "read never finished"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        got
    }
    #[test]
    fn aws_services_and_operations_come_from_the_installed_cli() {
        let (root, path) = aws_cli::fake_install("engine");
        let mut engine = Engine::new(path.clone(), root.clone());
        let mut at = |buffer: &str| aws_labels(&mut engine, buffer, &path);
        // Generated services next to the curated top-level commands.
        assert_eq!(at("aws r"), ["rds"]);
        assert!(at("aws ").contains(&"rds".to_string()) && at("aws ").contains(&"s3".to_string()));
        // Curated wins per operation, the rest is generated.
        let ec2 = at("aws ec2 describe-");
        assert!(
            ec2.contains(&"describe-hosts".to_string())
                && ec2.contains(&"describe-instances".to_string())
        );
        assert!(!at("aws ec2 old").contains(&"old-operation".to_string()));
        let rds = at("aws rds ");
        assert_eq!(
            rds.iter()
                .filter(|l| !l.starts_with('-'))
                .collect::<Vec<_>>(),
            ["describe-db-instances", "list-acls"]
        );
        assert!(at("aws s3api ").contains(&"get-bucket-foo".to_string()));
        // A curated operation keeps its flags and gains the generated ones; its own win.
        let curated = at("aws ec2 run-instances --");
        for flag in ["--count", "--min-count", "--cli-input-json"] {
            assert_eq!(
                curated.contains(&flag.to_string()),
                flag != "--min-count",
                "{flag}"
            );
        }
        // Flags before the service name do not hide it.
        let hosts = at("aws --profile dev ec2 describe-hosts --");
        for flag in [
            "--host-ids",
            "--dry-run",
            "--no-dry-run",
            "--max-results",
            "--cli-input-json",
            "--starting-token",
            "--page-size",
            "--region",
        ] {
            assert!(hosts.contains(&flag.to_string()), "{flag} in {hosts:?}");
        }
        assert!(
            !hosts.contains(&"--query".to_string())
                || hosts.iter().filter(|l| *l == "--query").count() == 1
        );
        assert!(
            at("aws ec2 describe-hosts --dry-run --")
                .iter()
                .all(|l| l != "--no-dry-run" && l != "--dry-run")
        );
        assert_eq!(at("aws ec2 describe-hosts --host-recovery "), ["off", "on"]);
        // Services the CLI lacks are rejected rather than skipped.
        assert!(at("aws banana ").is_empty());
        assert!(at("aws rds nope --").is_empty());
        engine.scan_path();
        let cwd = Path::new("/nonexistent-riga-test-directory");
        for (line, valid) in [
            (
                "aws rds describe-db-instances --db-instance-identifier x",
                true,
            ),
            ("aws ec2 describe-hosts --dry-run", true),
            ("aws rds describe-everything", false),
            ("aws banana describe", false),
        ] {
            assert_eq!(engine.valid_command(line, cwd), valid, "{line}");
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn aws_commands_are_not_validated_before_the_cli_data_is_read() {
        let (root, path) = aws_cli::fake_install("cold");
        let mut engine = Engine::new(path.clone(), root.clone());
        engine.scan_path();
        let cwd = Path::new("/nonexistent-riga-test-directory");
        let lines = [
            ("aws rds describe-db-instances", true),
            ("aws rds describe-dbinstances", false),
            ("aws rds nope", false),
            ("aws bogusservice x", false),
        ];
        // Nothing is built yet: the open curated spec must not wave anything through.
        for (line, _) in lines {
            assert!(!engine.valid_command(line, cwd), "cold: {line}");
        }
        aws_labels(&mut engine, "aws rds ", &path);
        for (line, valid) in lines {
            assert_eq!(engine.valid_command(line, cwd), valid, "warm: {line}");
        }
        // The service list is built but this service is not: still unverifiable.
        let mut engine = Engine::new(path.clone(), root.clone());
        engine.scan_path();
        aws_labels(&mut engine, "aws r", &path);
        assert!(!engine.valid_command("aws rds nope", cwd));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_changed_executable_is_a_new_install() {
        let (root, path) = aws_cli::fake_install("stamp");
        let cli = aws_cli::Cli::default();
        let curated = spec::parse("aws.riga", "command aws").unwrap();
        let stamp = |path: &str| cli.stamp(path, &curated).unwrap();
        let before = stamp(&path);
        let exe = root.join("aws-cli/v2/2.0.0/dist/aws");
        let file = fs::OpenOptions::new().write(true).open(exe).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        // A different PATH string forces the lookup to run again.
        assert_ne!(before, stamp(&format!("{path}:")));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn own_flags_rank_before_inherited_ones_and_truncation_drops_inherited_first() {
        let own: String = (0..140)
            .map(|i| format!("    flag --f{i:03} \"F\"\n"))
            .collect();
        let text = format!(
            "command r\n  flag --global|-g persistent \"G\"\n  command c\n    flag --aa-own \"O\"\n{own}  command d\n    flag --zed \"Z\"\n"
        );
        let nowhere = Path::new("/nonexistent-riga-test-directory");
        let mut engine = Engine::new(String::new(), nowhere.into());
        engine.specs = vec![spec::parse("r.riga", &text).unwrap()];
        // Alphabetically `--global` sorts before `--own`, but it is inherited.
        let got = labels(&mut engine, "r d --", nowhere);
        assert_eq!(got, ["--zed", "--global"]);
        let got = labels(&mut engine, "r c --", nowhere);
        assert_eq!(got.len(), MAX_RESULTS);
        assert!(got.contains(&"--aa-own".to_string()) && !got.contains(&"--global".to_string()));
        assert_eq!(labels(&mut engine, "r c --glo", nowhere), ["--global"]);
        // An exact match still wins.
        assert_eq!(labels(&mut engine, "r c --global", nowhere)[0], "--global");
    }
    #[test]
    fn aws_without_cli_data_stays_open_and_a_newer_install_is_picked_up() {
        let (root, path) = aws_cli::fake_install("open");
        let data = root.join("aws-cli/v2/2.0.0/dist/awscli");
        fs::rename(&data, root.join("hidden")).unwrap();
        let mut engine = Engine::new(path.clone(), root.clone());
        let mut at = |buffer: &str| aws_labels(&mut engine, buffer, &path);
        // Curated only: unknown services do not end completion; only global flags stay.
        assert!(at("aws ").contains(&"s3".to_string()) && !at("aws ").contains(&"rds".to_string()));
        assert_eq!(at("aws rds describe-db-instances --pro"), ["--profile"]);
        assert_eq!(at("aws rds --foo bar --out"), ["--output"]);
        assert!(!at("aws rds x --").contains(&"--version".to_string()));
        assert!(at("aws s3 ls --nope --").is_empty());
        engine.scan_path();
        let cwd = Path::new("/nonexistent-riga-test-directory");
        assert!(engine.valid_command("aws rds describe-db-instances", cwd));
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn misspelt_value_sources_are_errors_but_other_labels_are_text() {
        assert_eq!(
            error_of("command t\n  flag --a <git-brnach> \"A\""),
            "x.riga:2: unknown value source `<git-brnach>`"
        );
        assert!(error_of("command t\n  arg <aws-profle>").contains("<aws-profle>"));
        for ok in ["<git-branch>", "<branch-name>", "<git>", "<gitlike-x>"] {
            assert!(spec::parse("x.riga", &format!("command t\n  arg {ok}")).is_ok());
        }
    }
    #[test]
    fn flags_continue_after_flags_and_positionals_and_hide_used_aliases() {
        assert_eq!(flags("git status --short --b"), vec!["--branch"]);
        assert_eq!(flags("git status path/to/file -s -b"), vec!["-b"]);
        let available = flags("git status --short ");
        assert!(available.contains(&"--branch".into()));
        assert!(available.contains(&"-b".into()));
        assert!(!available.contains(&"--short".into()));
        assert!(!available.contains(&"-s".into()));
        let bundled = flags("git status -sb ");
        for used in ["-s", "--short", "-b", "--branch"] {
            assert!(!bundled.contains(&used.into()));
        }
        assert!(bundled.contains(&"--show-stash".into()));
        assert_eq!(flags("git diff --cached --stag"), Vec::<String>::new());
    }
    #[test]
    fn value_options_consume_exactly_one_argument_and_allow_repeats() {
        for input in [
            "git commit -m ",
            "git commit --message --",
            "git commit -am ",
        ] {
            assert!(complete_in(input, Path::new(".")).is_empty(), "{input}");
        }
        for input in [
            "git commit -m 'first message' --",
            "git commit --message=first --",
            "git commit -amfirst --",
            "git commit -m -- --",
            "git commit -m --amend --",
            "git commit -mmessage=content --",
        ] {
            let available = flags(input);
            assert!(available.contains(&"--message".into()), "{input}");
            assert!(available.contains(&"--amend".into()), "{input}");
        }
        let available = flags("git commit -amfirst -");
        assert!(!available.contains(&"--all".into()));
        assert!(!available.contains(&"-a".into()));
        assert!(available.contains(&"-m".into()));
        assert!(flags("git commit -Fmessage.txt --f").is_empty());
        assert!(flags("git commit --file=message.txt --f").is_empty());
    }
    #[test]
    fn option_terminator_and_unknown_flags_stop_catalog_guesses() {
        for input in [
            "git status -- --s",
            "git commit -m text -- --a",
            "git --unknown status --s",
            "git status --unknown --s",
            "git status -sx --b",
            "git -pCdirectory status --s",
        ] {
            assert!(flags(input).is_empty(), "{input}");
        }
    }
    #[test]
    fn global_options_and_configuration_values_preserve_subcommand_context() {
        for input in [
            "git -c color.ui=always status --s",
            "git -ccolor.ui=always status --s",
            "git --no-pager -c color.ui=always status --s",
        ] {
            assert!(flags(input).contains(&"--short".into()), "{input}");
        }
        let completions = complete_in("git -c status st", Path::new("."));
        assert!(
            completions
                .iter()
                .any(|suggestion| suggestion.label == "status")
        );
        assert!(complete_in("FOO=bar git st", Path::new(".")).is_empty());
    }
    #[test]
    fn file_values_directory_values_and_attached_unicode_replacement() {
        let root = std::env::temp_dir().join(format!("riga-catalog-files-{}", std::process::id()));
        fs::create_dir_all(root.join("repo/nested")).unwrap();
        fs::write(root.join("世界 write.txt"), "message").unwrap();
        fs::write(root.join("repo/nested/from-nested"), "").unwrap();
        fs::write(root.join("repo/from-repo"), "").unwrap();
        let dirs = complete_in("git -C ", &root);
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].label, "repo/");
        assert_eq!(complete_in("git -Cre", &root)[0].insert, "-Crepo/");
        for input in ["git commit -F 世", "git commit --file 世"] {
            assert_eq!(complete_in(input, &root)[0].insert, "'世界 write.txt'");
        }
        for input in [
            "git -C repo -C nested status from",
            "git -Crepo -Cnested status from",
        ] {
            assert_eq!(complete_in(input, &root)[0].label, "from-nested");
        }
        assert_eq!(
            complete_in("git -C repo -C '' status from", &root)[0].label,
            "from-repo"
        );
        assert!(complete_in("git -C '~/ambiguous' status from", &root).is_empty());
        let buffer = "git commit --file='世界 wr' --amend";
        let cursor = "git commit --file='世界 w".chars().count();
        let suggestions = Engine::new(String::new(), root.clone()).complete(&State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor,
            cwd: root.clone(),
            buffer: buffer.into(),
            path: String::new(),
        });
        let suggestion = &suggestions[0];
        assert_eq!(suggestion.insert, "--file='世界 write.txt'");
        let edited = format!(
            "{}{}{}",
            buffer.chars().take(suggestion.start).collect::<String>(),
            suggestion.insert,
            buffer.chars().skip(suggestion.end).collect::<String>()
        );
        assert_eq!(edited, "git commit --file='世界 write.txt' --amend");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn git_values_come_from_the_repository() {
        let root = std::env::temp_dir().join(format!("riga-git-values-{}", std::process::id()));
        let git = root.join("repo/.git");
        fs::create_dir_all(git.join("refs/heads")).unwrap();
        fs::create_dir_all(git.join("refs/remotes/origin")).unwrap();
        fs::create_dir_all(root.join("repo/src")).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        for r in ["heads/main", "heads/feat-a", "remotes/origin/feat-b"] {
            fs::write(git.join("refs").join(r), "0\n").unwrap();
        }
        let repo = root.join("repo");
        let labels = |buffer: &str, cwd: &Path| -> Vec<String> {
            complete_in(buffer, cwd)
                .into_iter()
                .map(|s| s.label)
                .collect()
        };
        // Branches rank before flags and paths; remote-only branches use their short name.
        assert_eq!(
            labels("git checkout ", &repo)[..3],
            ["feat-a", "feat-b", "main"]
        );
        assert!(labels("git checkout ", &repo).contains(&"src/".into()));
        assert_eq!(labels("git switch fe", &repo), ["feat-a", "feat-b"]);
        assert_eq!(
            labels("git rebase --onto origin/", &repo),
            ["origin/feat-b"]
        );
        assert_eq!(labels("git push ", &repo)[0], "origin");
        assert_eq!(labels("git push origin ma", &repo), ["main"]);
        // `-C` points discovery at another repository; outside one, nothing is generated.
        assert_eq!(labels("git -C repo switch ma", &root), ["main"]);
        assert!(labels("git switch ma", &root).is_empty());
        let insert = &complete_in("git restore --source=fe", &repo)[0];
        assert_eq!(insert.insert, "--source=feat-a");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn npm_values_come_from_package_json() {
        let root = std::env::temp_dir().join(format!("riga-npm-values-{}", std::process::id()));
        fs::create_dir_all(root.join("app/src")).unwrap();
        fs::write(
            root.join("app/package.json"),
            r#"{"scripts": {"build": "tsc", "build:watch": "tsc -w", "lint": "eslint ."},
                "devDependencies": {"eslint": "^9"}}"#,
        )
        .unwrap();
        let app = root.join("app");
        let labels = |buffer: &str, cwd: &Path| -> Vec<String> {
            complete_in(buffer, cwd)
                .into_iter()
                .map(|s| s.label)
                .collect()
        };
        // Scripts only, no paths such as `src/`.
        let run = labels("npm run ", &app);
        assert_eq!(run[..3], ["build", "build:watch", "lint"]);
        assert!(!run.contains(&"src/".into()));
        assert_eq!(labels("npm run-script bu", &app), ["build", "build:watch"]);
        let insert = &complete_in("npm run build:", &app)[0];
        assert_eq!(
            (insert.insert.as_str(), insert.kind),
            ("build:watch", "value")
        );
        assert_eq!(labels("npm rm es", &app), ["eslint"]);
        // `--prefix`/`-C` points at another project; outside one, nothing is generated.
        assert_eq!(labels("npm -C app run l", &root), ["lint"]);
        assert!(labels("npm run b", &root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ollama_models_come_from_manifests() {
        // complete_in uses the cwd as the engine's home.
        let home = std::env::temp_dir().join(format!("riga-ollama-values-{}", std::process::id()));
        let tag = home.join(".ollama/models/manifests/registry.ollama.ai/library/llama3/latest");
        fs::create_dir_all(tag.parent().unwrap()).unwrap();
        fs::write(&tag, "{}").unwrap();
        let labels = |buffer: &str| -> Vec<String> {
            complete_in(buffer, &home)
                .into_iter()
                .map(|s| s.label)
                .collect()
        };
        assert_eq!(labels("ollama run l"), ["llama3:latest"]);
        assert_eq!(labels("ollama rm llama3:latest l"), ["llama3:latest"]);
        assert_eq!(labels("ollama launch claude --model ll"), ["llama3:latest"]);
        assert!(labels("ollama pull ll").is_empty());
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn replacement_middle_unicode() {
        assert_eq!(
            context("cat 世界-file tail", 6),
            Some(Context {
                prefix: "世界".into(),
                start: 4,
                end: 11,
                previous: vec!["cat".into()],
                expand_tilde: false
            })
        );
    }
    #[test]
    fn quotes_and_escapes() {
        for input in ["cat 'my f", "cat \"my f", "cat my\\ f"] {
            assert_eq!(
                context(input, input.chars().count()).unwrap().prefix,
                "my f"
            );
        }
        assert_eq!(quote_word("it's here"), "'it'\\''s here'");
    }
    #[test]
    fn separators_and_empty() {
        assert_eq!(context("echo x; git st", 14).unwrap().previous, vec!["git"]);
        assert_eq!(context("cd ", 3).unwrap().start, 3);
        assert!(context("echo $(bad)", 11).is_none());
    }
    #[test]
    fn shell_lexer_edges() {
        assert_eq!(context("echo a\ngit st", 13).unwrap().previous, vec!["git"]);
        assert_eq!(context(r#"cat "a\q"#, 8).unwrap().prefix, "a\\q");
        assert!(!context("cat '~/foo", 10).unwrap().expand_tilde);
        assert!(context("cat ~/foo", 9).unwrap().expand_tilde);
        assert!(context("cat a*", 6).is_none());
        assert_eq!(quote_word("=ls"), "'=ls'");
        assert_eq!(quote_word("FOO=bar"), "'FOO=bar'");
    }
    #[test]
    fn executable_symlinks_follow_shell_path() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("riga-path-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("real-tool"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(root.join("real-tool"), fs::Permissions::from_mode(0o755)).unwrap();
        symlink(root.join("real-tool"), root.join("linked-tool")).unwrap();
        let mut engine = Engine::new(String::new(), root.clone());
        let state = State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor: 4,
            cwd: root.clone(),
            buffer: "link".into(),
            path: ".".into(),
        };
        assert!(
            engine
                .complete(&state)
                .iter()
                .any(|s| s.label == "linked-tool")
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn file_paths_and_git() {
        let root = std::env::temp_dir().join(format!("riga-test-{}", std::process::id()));
        fs::create_dir_all(root.join("hello world")).unwrap();
        let mut e = Engine::new(String::new(), root.clone());
        let mut s = State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor: 4,
            cwd: root.clone(),
            buffer: "cd h".into(),
            path: String::new(),
        };
        let v = e.complete(&s);
        assert_eq!(v[0].insert, "'hello world/'");
        s.buffer = "git st".into();
        s.cursor = 6;
        assert!(e.complete(&s).iter().any(|x| x.label == "status"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn npx_values_come_from_node_modules_and_package_json() {
        let root = std::env::temp_dir().join(format!("riga-npx-values-{}", std::process::id()));
        fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        fs::create_dir_all(root.join("packages/web")).unwrap();
        fs::write(root.join("node_modules/.bin/eslint"), "").unwrap();
        fs::write(root.join("packages/web/package.json"), "{}").unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"workspaces": ["packages/*"], "devDependencies": {"esbuild": "^0.24"}}"#,
        )
        .unwrap();
        let labels = |buffer: &str| -> Vec<String> {
            complete_in(buffer, &root)
                .into_iter()
                .map(|s| s.label)
                .collect()
        };
        // Local bins first; the command may still be a path.
        let bins = labels("npx ");
        assert_eq!(bins[0], "eslint");
        assert!(bins.contains(&"packages/".into()));
        assert_eq!(labels("npx -w "), ["packages/web"]);
        assert_eq!(labels("npx --workspace=p"), ["packages/web"]);
        assert_eq!(labels("npx -p es"), ["esbuild"]);
        // After the command, words belong to it.
        assert!(labels("npx eslint e").is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn vscode_values_come_from_home() {
        // `complete_in` uses the cwd as home.
        let home = std::env::temp_dir().join(format!("riga-vscode-values-{}", std::process::id()));
        fs::create_dir_all(
            home.join(".vscode/extensions/rust-lang.rust-analyzer-0.3.2-darwin-arm64"),
        )
        .unwrap();
        fs::create_dir_all(home.join("rust-dir")).unwrap();
        let labels = |buffer: &str| -> Vec<String> {
            complete_in(buffer, &home)
                .into_iter()
                .map(|s| s.label)
                .collect()
        };
        assert_eq!(
            labels("code --uninstall-extension ru"),
            ["rust-lang.rust-analyzer"]
        );
        assert_eq!(
            labels("code --disable-extension=r"),
            ["rust-lang.rust-analyzer"]
        );
        assert!(labels("code --profile ").is_empty());
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn uv_values_come_from_pyproject_and_tools_dir() {
        let root = std::env::temp_dir().join(format!("riga-uv-values-{}", std::process::id()));
        fs::create_dir_all(root.join("app/src")).unwrap();
        fs::create_dir_all(root.join("app/.local/share/uv/tools/ruff")).unwrap();
        fs::write(
            root.join("app/pyproject.toml"),
            "[project]\ndependencies = [\"requests>=2\"]\n[project.optional-dependencies]\ncli = [\"click\"]\n\
             [project.scripts]\nserve = \"app:main\"\n[dependency-groups]\ndev = [\"pytest\"]\n",
        )
        .unwrap();
        let app = root.join("app");
        let labels = |buffer: &str| -> Vec<String> {
            complete_in(buffer, &app)
                .into_iter()
                .map(|s| s.label)
                .collect()
        };
        assert_eq!(labels("uv remove ")[..3], ["click", "pytest", "requests"]);
        assert_eq!(labels("uv sync --group "), ["dev"]);
        assert_eq!(labels("uv run --extra=c"), ["cli"]);
        assert_eq!(labels("uv lock -P r"), ["requests"]);
        assert_eq!(labels("uv tool uninstall ")[..1], ["ruff"]);
        // uv run offers project scripts next to paths.
        let run = labels("uv run s");
        assert!(run.contains(&"serve".into()) && run.contains(&"src/".into()));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn kubectl_and_cargo_installed_use_the_engine_home_not_the_process_environment() {
        let home = std::env::temp_dir().join(format!("riga-home-values-{}", std::process::id()));
        fs::create_dir_all(home.join(".kube")).unwrap();
        fs::create_dir_all(home.join(".cargo")).unwrap();
        fs::write(
            home.join(".kube/config"),
            "contexts:\n- context:\n    cluster: c\n    extensions:\n    - extension:\n        v: 1\n      name: context_info\n    namespace: web\n    user: u\n  name: minikube\ncurrent-context: minikube\n",
        )
        .unwrap();
        fs::write(
            home.join(".cargo/.crates.toml"),
            "[v1]\n\"ripgrep 14.1.0 (registry+https://example.invalid)\" = [\"rg\"]\n",
        )
        .unwrap();
        let described = |buffer: &str| -> Vec<(String, String)> {
            complete_in(buffer, &home)
                .into_iter()
                .map(|s| (s.label, s.description))
                .collect()
        };
        assert_eq!(
            described("kubectl --context "),
            [("minikube".into(), "Current kubeconfig context".into())]
        );
        fs::create_dir_all(home.join(".aws")).unwrap();
        fs::write(home.join(".aws/config"), "[profile dev]\n").unwrap();
        fs::write(home.join(".aws/credentials"), "[ci]\n").unwrap();
        let profiles: Vec<_> = described("aws s3 ls --profile ")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(profiles, ["ci", "dev"]);
        assert_eq!(described("aws --profile=d")[0].0, "dev");
        let namespaces = described("kubectl get pods -n ");
        assert_eq!(namespaces[0].0, "web");
        assert_eq!(
            described("cargo uninstall ")[0],
            ("ripgrep".into(), "14.1.0 rg".into())
        );
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn cdk_profiles_and_stacks_come_from_local_files() {
        let root = std::env::temp_dir().join(format!("riga-cdk-values-{}", std::process::id()));
        fs::create_dir_all(root.join(".aws")).unwrap();
        fs::create_dir_all(root.join("cdk.out")).unwrap();
        fs::write(root.join(".aws/config"), "[profile dev]\n").unwrap();
        fs::write(
            root.join("cdk.out/manifest.json"),
            r#"{"artifacts": {"Api": {"type": "aws:cloudformation:stack"}}}"#,
        )
        .unwrap();
        let labels = |buffer: &str| -> Vec<String> {
            complete_in(buffer, &root)
                .into_iter()
                .map(|s| s.label)
                .collect()
        };
        assert_eq!(labels("cdk deploy --profile "), ["dev"]);
        assert_eq!(labels("cdk --profile dev diff A"), ["Api"]);
        assert_eq!(labels("cdk destroy Api A"), ["Api"]);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rustup_toolchains_come_from_home() {
        let home = std::env::temp_dir().join(format!("riga-rustup-engine-{}", std::process::id()));
        fs::create_dir_all(home.join(".rustup/toolchains/nightly-x")).unwrap();
        let mut e = Engine::new(String::new(), home.clone());
        let s = State {
            session: "s".into(),
            pane: "p".into(),
            revision: 1,
            cursor: 15,
            cwd: home.clone(),
            buffer: "rustup default ".into(),
            path: String::new(),
        };
        let labels: Vec<_> = e.complete(&s).into_iter().map(|x| x.label).collect();
        assert!(labels.contains(&"nightly-x".into()) && labels.contains(&"stable".into()));
        assert!(!labels.contains(&".rustup/".into()));
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn brew_values_come_from_the_prefix_on_shell_path() {
        let root = std::env::temp_dir().join(format!("riga-brew-values-{}", std::process::id()));
        let prefix = root.join("hb");
        for dir in [
            "bin",
            "Cellar/wget/1",
            "Caskroom/firefox",
            "var/homebrew/pinned/wget",
        ] {
            fs::create_dir_all(prefix.join(dir)).unwrap();
        }
        fs::write(prefix.join("bin/brew"), "").unwrap();
        fs::create_dir_all(root.join("wdir")).unwrap();
        let labels = |buffer: &str| -> Vec<String> {
            Engine::new(String::new(), root.clone())
                .complete(&State {
                    session: "s".into(),
                    pane: "p".into(),
                    revision: 1,
                    cursor: buffer.chars().count(),
                    cwd: root.clone(),
                    buffer: buffer.into(),
                    path: prefix.join("bin").to_string_lossy().into_owned(),
                })
                .into_iter()
                .filter(|s| s.kind != "flag")
                .map(|s| s.label)
                .collect()
        };
        // Names only, no paths such as `wdir/`.
        assert_eq!(labels("brew uninstall "), ["firefox", "wget"]);
        assert_eq!(labels("brew rm --zap w"), ["wget"]);
        assert_eq!(labels("brew unpin "), ["wget"]);
        assert_eq!(labels("brew services start "), ["wget"]);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cargo_values_come_from_cargo_toml() {
        let root = std::env::temp_dir().join(format!("riga-cargo-values-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("app/src/bin")).unwrap();
        fs::write(
            root.join("app/Cargo.toml"),
            "[package]\nname = \"app\"\n[features]\nfast = []\n[dependencies]\nserde = \"1\"\n\
             [profile.ci]\ninherits = \"dev\"\n",
        )
        .unwrap();
        fs::write(root.join("app/src/main.rs"), "").unwrap();
        fs::write(root.join("app/src/bin/helper.rs"), "").unwrap();
        let app = root.join("app");
        let labels = |buffer: &str, cwd: &Path| -> Vec<String> {
            complete_in(buffer, cwd)
                .into_iter()
                .map(|s| s.label)
                .collect()
        };
        // Names only, no paths such as `src/`.
        assert_eq!(labels("cargo build -p ", &app), ["app"]);
        assert_eq!(labels("cargo run --bin ", &app), ["app", "helper"]);
        assert_eq!(labels("cargo b -F f", &app), ["fast"]);
        assert_eq!(labels("cargo rm s", &app), ["serde"]);
        assert_eq!(labels("cargo build --profile c", &app), ["ci"]);
        let insert = &complete_in("cargo build --features=fa", &app)[0];
        assert_eq!(insert.insert, "--features=fast");
        // `-C` points at another package; outside one, nothing is generated.
        assert_eq!(labels("cargo -C app run --bin h", &root), ["helper"]);
        assert!(labels("cargo build -p ", &root).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
