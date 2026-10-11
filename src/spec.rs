//! Declarative `.riga` completion specs. Parsing is pure data: specs never run code or subprocesses.
use crate::sources::Source;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::Path,
};
pub const MAX_SPEC_BYTES: u64 = 1 << 20;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Text(String),
    File,
    Directory,
    Choices(Vec<String>),
    /// A built-in local source (see `sources.rs`).
    Source(Source),
}
impl Value {
    /// Whether file system paths are offered next to the value's own candidates. `is_flag` tells
    /// a flag's value from a positional.
    pub fn offers_paths(&self, is_flag: bool) -> bool {
        match self {
            Value::File | Value::Directory => true,
            Value::Choices(_) => false,
            // Flag text values have no source; text positionals (e.g. <branch>) often name paths too.
            Value::Text(_) => !is_flag,
            Value::Source(source) => source.offers_paths(is_flag),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueSpec {
    pub value: Value,
    pub optional: bool,
}
#[derive(Clone, Debug)]
pub struct Flag {
    pub names: Vec<String>,
    pub value: Option<ValueSpec>,
    pub repeat: bool,
    pub persistent: bool,
    pub chdir: bool,
    pub requires: Vec<usize>,
    pub excludes: Vec<usize>,
    pub description: String,
    pub command: usize,
}
#[derive(Clone, Debug)]
pub struct Arg {
    pub value: ValueSpec,
    pub repeat: bool,
    pub stop: bool,
    pub description: String,
}
#[derive(Clone, Debug, Default)]
pub struct Command {
    pub names: Vec<String>,
    pub nobundle: bool,
    /// Words that are not subcommands do not end completion; see `docs/SPECS.md`.
    pub open: bool,
    pub description: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub flags: Vec<usize>,
    /// Spelling -> index into `Spec::flags` for this command's own flags.
    pub flag_names: HashMap<String, usize>,
    /// Spelling -> index into `Spec::commands` for this command's children.
    pub child_names: HashMap<String, usize>,
    pub args: Vec<Arg>,
}
#[derive(Clone, Debug)]
pub struct Test {
    pub line: usize,
    pub buffer: String,
    pub expect: Vec<(bool, String)>,
}
/// `commands[0]` is always the root executable.
#[derive(Clone, Debug, Default)]
pub struct Spec {
    pub commands: Vec<Command>,
    pub flags: Vec<Flag>,
    pub tests: Vec<Test>,
}
impl Spec {
    pub fn name(&self) -> &str {
        &self.commands[0].names[0]
    }
    /// Current command's flags, then ancestors' persistent flags; earlier entries shadow later spellings.
    pub fn visible_flags(&self, command: usize) -> Vec<usize> {
        let mut out = vec![];
        let mut current = Some(command);
        while let Some(c) = current {
            let flags = self.commands[c].flags.iter();
            out.extend(flags.filter(|&&f| c == command || self.flags[f].persistent));
            current = self.commands[c].parent;
        }
        out
    }
    /// Resolve a spelling visible in `command`: its own flags, then ancestors' persistent flags.
    pub fn flag(&self, command: usize, name: &str) -> Option<usize> {
        let mut current = Some(command);
        while let Some(c) = current {
            if let Some(&f) = self.commands[c].flag_names.get(name)
                && (c == command || self.flags[f].persistent)
            {
                return Some(f);
            }
            current = self.commands[c].parent;
        }
        None
    }
}
/// Read a bounded spec file. File-level errors are `path: message`, parse errors `path:line: message`.
pub fn read(path: &Path) -> Result<Spec, Vec<String>> {
    let label = printable(&path.display().to_string());
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return Err(vec![format!("{label}: not a readable regular file")]);
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_SPEC_BYTES + 1).read_to_string(&mut text))
        .map_err(|e| vec![format!("{label}: {e}")])?;
    if text.len() as u64 > MAX_SPEC_BYTES {
        return Err(vec![format!("{label}: spec exceeds 1 MiB")]);
    }
    parse(&label, &text)
}
// Loading and lookups are hash-indexed and linear in the 1 MiB input; only depth multiplies
// per-word ancestor walks, so it is the one structural cap.
const MAX_DEPTH: usize = 32;
const MAX_ERRORS: usize = 100;
// Displayed text: every spelling is listed in each suggestion's description.
const MAX_NAMES: usize = 64;
const MAX_DESCRIPTION: usize = 200;
const MAX_NAME: usize = 128;
/// Escape characters that could act on the terminal when an error echoes untrusted spec text.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if deceptive(c) {
                c.escape_unicode().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}
/// Characters that could disguise popup text: controls, bidi overrides and line separators.
pub(crate) fn deceptive(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}'
        )
        || ('\u{202a}'..='\u{202e}').contains(&c)
        || ('\u{2066}'..='\u{2069}').contains(&c)
}
struct Ref {
    line: usize,
    flag: usize,
    excludes: bool,
    name: String,
}
/// A flag as the `.riga` parser or a generator describes it. `requires` and `excludes` name other
/// flags visible in the same command and are resolved in `finish`.
#[derive(Clone, Debug, Default)]
pub struct FlagDef {
    pub names: Vec<String>,
    pub value: Option<ValueSpec>,
    pub repeat: bool,
    pub persistent: bool,
    pub chdir: bool,
    pub requires: Vec<String>,
    pub excludes: Vec<String>,
    pub description: String,
}
/// Assembles a `Spec` for the `.riga` parser and for generated specs alike, so both obey the same
/// limits: name charset and length, description length and characters, duplicate spellings,
/// choices, and `requires`/`excludes` resolution.
#[derive(Default)]
pub struct Builder {
    spec: Spec,
    label: String,
    refs: Vec<Ref>,
    errors: Vec<(usize, String)>,
    total: usize,
}
impl Builder {
    /// `label` prefixes the errors `finish` returns.
    pub fn new(label: &str) -> Self {
        Builder {
            label: label.into(),
            ..Builder::default()
        }
    }
    /// Continue from a finished spec; its references are already resolved.
    pub fn from_spec(spec: Spec, label: &str) -> Self {
        Builder {
            spec,
            ..Builder::new(label)
        }
    }
    pub fn spec(&self) -> &Spec {
        &self.spec
    }
    pub fn set_open(&mut self, command: usize, open: bool) {
        self.spec.commands[command].open = open;
    }
    /// Keep only the earliest errors, but count them all.
    fn report(&mut self, line: usize, message: String) {
        self.total += 1;
        if self.errors.len() < 2 * MAX_ERRORS {
            self.errors.push((line, message));
        }
    }
    /// `parent` is `None` only for the one root command.
    pub fn command(
        &mut self,
        parent: Option<usize>,
        names: Vec<String>,
        nobundle: bool,
        open: bool,
        description: String,
    ) -> Result<usize, String> {
        check_description(&description)?;
        check_names(&names, false)?;
        let spec = &mut self.spec;
        let index = spec.commands.len();
        match parent {
            None if index > 0 => return Err("the top level must be exactly one `command`".into()),
            Some(p) => {
                let own = &mut spec.commands[p].child_names;
                if let Some(n) = names.iter().find(|n| own.contains_key(*n)) {
                    return Err(format!("duplicate command `{n}`"));
                }
                own.extend(names.iter().map(|n| (n.clone(), index)));
                spec.commands[p].children.push(index);
            }
            None => {}
        }
        spec.commands.push(Command {
            names,
            nobundle,
            open,
            description,
            parent,
            ..Command::default()
        });
        Ok(index)
    }
    /// `line` is only used to place reference errors.
    pub fn flag(&mut self, parent: usize, line: usize, def: FlagDef) -> Result<usize, String> {
        check_description(&def.description)?;
        check_names(&def.names, true)?;
        if let Some(ValueSpec {
            value: Value::Choices(choices),
            ..
        }) = &def.value
            && !valid_choices(choices)
        {
            return Err(format!(
                "choices must be non-empty words in `{{{}}}`",
                choices.join(",")
            ));
        }
        let mut targets = HashSet::new();
        for (kind, list) in [("requires", &def.requires), ("excludes", &def.excludes)] {
            for name in list {
                if !name.starts_with('-') {
                    return Err(format!("`{kind}` needs flag names, got `{name}`"));
                }
                once(&mut targets, name, "reference")?;
            }
        }
        if def.chdir
            && def
                .value
                .as_ref()
                .is_none_or(|v| v.value != Value::Directory)
        {
            return Err("`chdir` requires a <directory> value".into());
        }
        let spec = &mut self.spec;
        let own = &mut spec.commands[parent].flag_names;
        if let Some(n) = def.names.iter().find(|n| own.contains_key(*n)) {
            return Err(format!("duplicate flag `{n}`"));
        }
        let index = spec.flags.len();
        own.extend(def.names.iter().map(|n| (n.clone(), index)));
        spec.commands[parent].flags.push(index);
        for (excludes, list) in [(false, &def.requires), (true, &def.excludes)] {
            for name in list {
                self.refs.push(Ref {
                    line,
                    flag: index,
                    excludes,
                    name: name.clone(),
                });
            }
        }
        spec.flags.push(Flag {
            names: def.names,
            value: def.value,
            repeat: def.repeat,
            persistent: def.persistent,
            chdir: def.chdir,
            requires: vec![],
            excludes: vec![],
            description: def.description,
            command: parent,
        });
        Ok(index)
    }
    pub fn arg(&mut self, parent: usize, arg: Arg) -> Result<(), String> {
        check_description(&arg.description)?;
        let args = &mut self.spec.commands[parent].args;
        if args.last().is_some_and(|a| a.repeat) {
            return Err("a `repeat` arg must be the last arg".into());
        }
        args.push(arg);
        Ok(())
    }
    /// Resolve references and return the spec, or the errors as `label:line: message`, sorted by
    /// line, at most `MAX_ERRORS`.
    pub fn finish(mut self) -> Result<Spec, Vec<String>> {
        // A failed root line already explains a missing root.
        if self.spec.commands.is_empty() && self.errors.is_empty() {
            self.report(1, "missing top-level `command`".into());
        }
        for r in std::mem::take(&mut self.refs) {
            // Refs are only recorded for flags that were added, but never index untrusted positions blindly.
            let Some(flag) = self.spec.flags.get(r.flag) else {
                continue;
            };
            let found = self.spec.flag(flag.command, &r.name);
            let spec = &mut self.spec;
            match found {
                Some(target) if target == r.flag => {
                    self.report(r.line, "flag cannot reference itself".into())
                }
                Some(target) if r.excludes => {
                    spec.flags[r.flag].excludes.push(target);
                    spec.flags[target].excludes.push(r.flag);
                }
                Some(target) => spec.flags[r.flag].requires.push(target),
                None => self.report(r.line, format!("`{}` is not a flag visible here", r.name)),
            }
        }
        for flag in &mut self.spec.flags {
            flag.excludes.sort_unstable();
            flag.excludes.dedup();
        }
        if self.errors.is_empty() {
            return Ok(self.spec);
        }
        let label = &self.label;
        let mut errors = self.errors;
        errors.sort_by_key(|(line, _)| *line);
        let extra = self.total - errors.len().min(MAX_ERRORS);
        let mut out: Vec<String> = errors
            .into_iter()
            .take(MAX_ERRORS)
            .map(|(line, m)| printable(&format!("{label}:{line}: {m}")))
            .collect();
        if extra > 0 {
            out.push(printable(&format!(
                "{label}: {extra} more errors not shown"
            )));
        }
        Err(out)
    }
}
fn check_description(description: &str) -> Result<(), String> {
    if description.chars().count() > MAX_DESCRIPTION {
        return Err(format!(
            "descriptions are limited to {MAX_DESCRIPTION} characters"
        ));
    }
    if description.chars().any(deceptive) {
        return Err("descriptions must not contain control or bidirectional characters".into());
    }
    Ok(())
}
/// Plain text for a generated description: no control or bidirectional characters, whitespace
/// collapsed, cut to `max` characters (at most `MAX_DESCRIPTION`) with an ellipsis.
pub fn clamp_description(text: &str, max: usize) -> String {
    let clean: String = text.chars().filter(|&c| !deceptive(c)).collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let max = max.min(MAX_DESCRIPTION);
    if clean.chars().count() <= max {
        return clean;
    }
    let cut: String = clean.chars().take(max - 1).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.']))
}
fn valid_choices(choices: &[String]) -> bool {
    !choices
        .iter()
        .any(|c| c.is_empty() || c.contains(['{', '}', '[', ']']) || c.chars().any(deceptive))
}
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Command(usize),
    Leaf,
    /// A rejected command: its children are skipped silently instead of cascading errors.
    Skip,
}
#[derive(Default)]
struct Parser {
    b: Builder,
    /// Statement at each depth; only commands may have children.
    stack: Vec<Slot>,
}
pub fn parse(label: &str, text: &str) -> Result<Spec, Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Parser {
        b: Builder::new(label),
        stack: vec![],
    };
    for (n, raw) in text.lines().enumerate() {
        if let Err(m) = p.line(n + 1, raw) {
            p.b.report(n + 1, m);
        }
    }
    p.b.finish()
}
fn once(seen: &mut HashSet<String>, item: &str, what: &str) -> Result<(), String> {
    if seen.insert(item.into()) {
        Ok(())
    } else {
        Err(format!("duplicate {what} `{item}`"))
    }
}
impl Parser {
    fn line(&mut self, number: usize, raw: &str) -> Result<(), String> {
        let spec = &mut self.b.spec;
        let body = raw.trim_start_matches(' ');
        if body.starts_with('\t') {
            return Err("tabs are not allowed in indentation".into());
        }
        if let Some(rest) = body.strip_prefix("#test") {
            let rest = rest.strip_prefix(' ').ok_or(TEST_USAGE)?;
            return test(spec, number, rest);
        }
        let (words, description) = tokenize(body)?;
        let Some((keyword, rest)) = words.split_first() else {
            return match description {
                Some(_) => Err("a description must follow a statement".into()),
                None => Ok(()),
            };
        };
        let indent = raw.len() - body.len();
        if !indent.is_multiple_of(2) {
            return Err("indentation must be a multiple of two spaces".into());
        }
        let depth = indent / 2;
        if depth > self.stack.len() {
            return Err("indentation may grow by only one level".into());
        }
        if depth > MAX_DEPTH {
            return Err(format!("nesting deeper than {MAX_DEPTH} levels"));
        }
        self.stack.truncate(depth);
        let parent = match (depth, self.stack.last()) {
            (0, _) if spec.commands.is_empty() && keyword == "command" => None,
            (0, _) => return Err("the top level must be exactly one `command`".into()),
            (_, Some(Slot::Command(c))) => Some(*c),
            (_, Some(Slot::Skip)) => {
                self.stack.push(Slot::Skip);
                return Ok(());
            }
            _ => return Err("only commands may have children".into()),
        };
        let description = description.unwrap_or_default();
        check_description(&description)?;
        let result = self.statement(number, keyword, rest, description, parent);
        if result.is_err() && keyword == "command" {
            self.stack.push(Slot::Skip);
        }
        result
    }
    fn statement(
        &mut self,
        number: usize,
        keyword: &str,
        rest: &[String],
        description: String,
        parent: Option<usize>,
    ) -> Result<(), String> {
        let mut rest = rest.iter().map(String::as_str);
        let mut attrs = HashSet::new();
        match keyword {
            "command" => {
                let names = names(rest.next(), false)?;
                let (mut nobundle, mut open) = (false, false);
                for attr in rest {
                    match attr {
                        "nobundle" => nobundle = true,
                        "open" => open = true,
                        _ => return Err(format!("unknown command attribute `{attr}`")),
                    }
                    once(&mut attrs, attr, "attribute")?;
                }
                let index = self.b.command(parent, names, nobundle, open, description)?;
                self.stack.push(Slot::Command(index));
            }
            "flag" => {
                let p = parent.ok_or("the top level must be exactly one `command`")?;
                let names = names(rest.next(), true)?;
                let mut rest = rest.peekable();
                let value = match rest.next_if(|t| t.starts_with(['<', '{', '['])) {
                    Some(t) => Some(value(t)?),
                    None => None,
                };
                let mut def = FlagDef {
                    names,
                    value,
                    description,
                    ..FlagDef::default()
                };
                let mut targets = HashSet::new();
                for attr in rest {
                    let (key, list) = attr.split_once('=').unwrap_or((attr, ""));
                    once(&mut attrs, key, "attribute")?;
                    match (key, attr.contains('=')) {
                        ("repeat", false) => def.repeat = true,
                        ("persistent", false) => def.persistent = true,
                        ("chdir", false) => def.chdir = true,
                        (kind @ ("requires" | "excludes"), true) => {
                            for name in list.split(',') {
                                if !name.starts_with('-') {
                                    return Err(format!("`{kind}` needs flag names, got `{name}`"));
                                }
                                once(&mut targets, name, "reference")?;
                                let list = if kind == "requires" {
                                    &mut def.requires
                                } else {
                                    &mut def.excludes
                                };
                                list.push(name.into());
                            }
                        }
                        _ => return Err(format!("unknown flag attribute `{attr}`")),
                    }
                }
                self.b.flag(p, number, def)?;
                self.stack.push(Slot::Leaf);
            }
            "arg" => {
                let p = parent.ok_or("the top level must be exactly one `command`")?;
                let mut arg = Arg {
                    value: value(rest.next().ok_or("`arg` needs a value such as <file>")?)?,
                    repeat: false,
                    stop: false,
                    description,
                };
                for attr in rest {
                    match attr {
                        "repeat" => arg.repeat = true,
                        "stop" => arg.stop = true,
                        _ => return Err(format!("unknown arg attribute `{attr}`")),
                    }
                    once(&mut attrs, attr, "attribute")?;
                }
                self.b.arg(p, arg)?;
                self.stack.push(Slot::Leaf);
            }
            _ => return Err(format!("unknown keyword `{keyword}`")),
        }
        Ok(())
    }
}
fn names(token: Option<&str>, flag: bool) -> Result<Vec<String>, String> {
    let names: Vec<String> = token
        .ok_or("missing name")?
        .split('|')
        .map(Into::into)
        .collect();
    check_names(&names, flag)?;
    Ok(names)
}
fn check_names(names: &[String], flag: bool) -> Result<(), String> {
    let mut seen = HashSet::new();
    for n in names {
        if n.chars().count() > MAX_NAME {
            return Err(format!("names are limited to {MAX_NAME} characters"));
        }
        // `#` only after the first character, where it can't start a comment (curl's `-#`).
        let valid = !n.is_empty()
            && !n.starts_with('#')
            && n.chars()
                .all(|c| c.is_alphanumeric() || "-_.:+@%/#".contains(c));
        if !valid {
            return Err(format!("invalid name `{n}`"));
        } else if flag && (!n.starts_with('-') || n == "-" || n == "--") {
            return Err(format!("flag names must start with `-`, got `{n}`"));
        } else if !flag && n.starts_with('-') {
            return Err(format!("command names must not start with `-`, got `{n}`"));
        }
        once(&mut seen, n, "name")?;
        if seen.len() > MAX_NAMES {
            return Err(format!("more than {MAX_NAMES} names"));
        }
    }
    Ok(())
}
/// Not recursive: at most one `[...]` wrapper, checked before the inner value is parsed.
fn value(token: &str) -> Result<ValueSpec, String> {
    let (token, optional) = match token.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        Some(inner) if inner.starts_with('[') => {
            return Err("optional values cannot be nested".into());
        }
        Some(inner) => (inner, true),
        None => (token, false),
    };
    let value = if let Some(label) = token.strip_prefix('<').and_then(|t| t.strip_suffix('>')) {
        if label.is_empty() || label.contains(['<', '>', '{', '}', '[', ']', ',']) {
            return Err(format!("invalid value label `{token}`"));
        }
        match label {
            "file" => Value::File,
            "directory" => Value::Directory,
            _ => match Source::parse(label) {
                Some(source) => Value::Source(source),
                // A misspelt source would otherwise silently complete nothing.
                None if Source::family(label) => {
                    return Err(format!("unknown value source `{token}`"));
                }
                None => Value::Text(label.into()),
            },
        }
    } else if let Some(list) = token.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
        let choices: Vec<String> = list.split(',').map(str::to_owned).collect();
        if !valid_choices(&choices) {
            return Err(format!("choices must be non-empty words in `{token}`"));
        }
        Value::Choices(choices)
    } else {
        return Err(format!("invalid value `{token}`"));
    };
    Ok(ValueSpec { value, optional })
}
/// Words, then an optional final quoted description. `#` starts a comment at the start of a token.
fn tokenize(line: &str) -> Result<(Vec<String>, Option<String>), String> {
    let mut words = vec![];
    let mut description = None;
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == ' ' || c == '\t' {
            chars.next();
            continue;
        }
        if c == '#' {
            break;
        }
        if description.is_some() {
            return Err("the description must be the last token".into());
        }
        if c == '"' {
            chars.next();
            description = Some(quoted(&mut chars)?);
            continue;
        }
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            match c {
                ' ' | '\t' => break,
                '"' => return Err("unexpected `\"` inside a word".into()),
                _ => word.push(c),
            }
            chars.next();
        }
        words.push(word);
    }
    Ok((words, description))
}
fn quoted(chars: &mut impl Iterator<Item = char>) -> Result<String, String> {
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Ok(out),
            '\\' => match chars.next() {
                Some(e @ ('"' | '\\')) => out.push(e),
                _ => return Err("only \\\" and \\\\ escapes are allowed in quotes".into()),
            },
            _ => out.push(c),
        }
    }
    Err("unterminated quoted string".into())
}
const TEST_USAGE: &str = "lines starting with #test must read: #test \"buffer\" => +shown -absent";
/// `#test "<buffer>" => +shown -hidden`; the first character of each expectation is its sign.
fn test(spec: &mut Spec, line: usize, rest: &str) -> Result<(), String> {
    let mut chars = rest.chars();
    if chars.next() != Some('"') {
        return Err(TEST_USAGE.into());
    }
    let buffer = quoted(&mut chars)?;
    let tail = chars.as_str().trim_start();
    let expect: Vec<(bool, String)> = tail
        .strip_prefix("=>")
        .ok_or("#test needs `=>` after the buffer")?
        .split_whitespace()
        .map(
            |t| match t.split_at(t.chars().next().map_or(0, char::len_utf8)) {
                (sign @ ("+" | "-"), label) if !label.is_empty() => Ok((sign == "+", label.into())),
                _ => Err(format!("expectation `{t}` must be +label or -label")),
            },
        )
        .collect::<Result<_, _>>()?;
    if expect.is_empty() {
        return Err("#test needs at least one expectation".into());
    }
    spec.tests.push(Test {
        line,
        buffer,
        expect,
    });
    Ok(())
}
