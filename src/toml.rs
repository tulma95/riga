//! Bounded reader for the TOML subset that Cargo and uv manifests use: tables, arrays of tables,
//! dotted and quoted keys, basic/literal/multi-line strings, arrays and (multi-line) inline tables.
//! Non-string scalars are skipped unvalidated and table redefinition rules are not checked.
//! `parse` never panics and always returns the document built before the first error.
use std::collections::BTreeMap;
const MAX_DEPTH: usize = 32;
const MAX_KEYS: usize = 200_000;
pub type Table = BTreeMap<String, Value>;
#[derive(Debug, PartialEq)]
pub enum Value {
    Str(String),
    Array(Vec<Value>),
    Table(Table),
    /// Integers, floats, booleans and dates.
    Other,
}
impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Table(t) => t.get(key),
            _ => None,
        }
    }
    pub fn at(&self, path: &[&str]) -> Option<&Value> {
        path.iter().try_fold(self, |v, key| v.get(key))
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    /// Array elements; empty for any other type.
    pub fn items(&self) -> &[Value] {
        match self {
            Value::Array(a) => a,
            _ => &[],
        }
    }
    /// The string elements of an array.
    pub fn strings(&self) -> impl Iterator<Item = &str> {
        self.items().iter().filter_map(Value::as_str)
    }
    /// Key/value pairs sorted by key; empty for any other type.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &Value)> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.as_str(), v))
    }
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries().map(|(k, _)| k)
    }
    /// The string at `path`, or the `name` of each table in the array of tables at `path`.
    pub fn names_at(&self, path: &[&str]) -> Vec<&str> {
        self.at(path)
            .map(|v| {
                v.items()
                    .iter()
                    .filter_map(|t| t.get("name")?.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }
}
/// The document as a table value; after an error it holds everything parsed before it.
pub fn parse(text: &str) -> Value {
    let mut parser = Parser {
        s: text.as_bytes(),
        i: 0,
        keys: 0,
    };
    let mut root = Table::new();
    let _ = parser.document(&mut root);
    Value::Table(root)
}
struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    keys: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: u8) -> bool {
        (self.peek() == Some(c)).then(|| self.i += 1).is_some()
    }
    fn starts(&self, pat: &[u8]) -> bool {
        self.s[self.i..].starts_with(pat)
    }
    /// Skips spaces and comments, plus newlines when `lines` is set.
    fn blank(&mut self, lines: bool) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t') => self.i += 1,
                Some(b'\n' | b'\r') if lines => self.i += 1,
                Some(b'#') => {
                    while !matches!(self.peek(), Some(b'\n') | None) {
                        self.i += 1;
                    }
                }
                _ => return,
            }
        }
    }
    fn document(&mut self, root: &mut Table) -> Option<()> {
        let mut current: Vec<String> = vec![];
        loop {
            self.blank(true);
            if self.peek().is_none() {
                return Some(());
            }
            if self.eat(b'[') {
                let array = self.eat(b'[');
                let path = self.key()?;
                (self.eat(b']') && (!array || self.eat(b']'))).then_some(())?;
                self.count()?;
                if array {
                    let (last, parent) = path.split_last()?;
                    let table = descend(root, parent)?;
                    match table
                        .entry(last.clone())
                        .or_insert_with(|| Value::Array(vec![]))
                    {
                        Value::Array(items) => items.push(Value::Table(Table::new())),
                        _ => return None,
                    }
                } else {
                    descend(root, &path)?;
                }
                current = path;
            } else {
                let key = self.key()?;
                self.blank(false);
                self.eat(b'=').then_some(())?;
                self.blank(false);
                let value = self.value(0)?;
                self.count()?;
                let (last, parent) = key.split_last()?;
                let table = descend(descend(root, &current)?, parent)?;
                table.insert(last.clone(), value);
            }
            self.blank(false);
            matches!(self.peek(), Some(b'\n' | b'\r') | None).then_some(())?;
        }
    }
    fn count(&mut self) -> Option<()> {
        self.keys += 1;
        (self.keys <= MAX_KEYS).then_some(())
    }
    fn key(&mut self) -> Option<Vec<String>> {
        let mut parts = vec![];
        loop {
            self.blank(false);
            let part = match self.peek()? {
                b'"' | b'\'' => self.string(false)?,
                _ => {
                    let start = self.i;
                    while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                    {
                        self.i += 1;
                    }
                    (self.i > start).then_some(())?;
                    String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
                }
            };
            parts.push(part);
            if parts.len() > MAX_DEPTH {
                return None;
            }
            self.blank(false);
            if !self.eat(b'.') {
                return Some(parts);
            }
        }
    }
    fn hex(&mut self, digits: usize) -> Option<char> {
        let hex = std::str::from_utf8(self.s.get(self.i..self.i + digits)?).ok()?;
        self.i += digits;
        char::from_u32(u32::from_str_radix(hex, 16).ok()?)
    }
    fn string(&mut self, multiline_ok: bool) -> Option<String> {
        let q = self.peek()?;
        let triple = [q; 3];
        let multi = multiline_ok && self.starts(&triple);
        let delimiter = if multi { &triple[..] } else { &triple[..1] };
        self.i += delimiter.len();
        if multi {
            // A newline right after the opening delimiter is dropped.
            if self.starts(b"\r\n") {
                self.i += 2;
            } else {
                self.eat(b'\n');
            }
        }
        let mut out = vec![];
        loop {
            if self.starts(delimiter) {
                // Up to two quotes may directly precede the closing delimiter in a multi-line string.
                while multi && self.s.get(self.i + 3) == Some(&q) {
                    out.push(q);
                    self.i += 1;
                }
                self.i += delimiter.len();
                return Some(String::from_utf8_lossy(&out).into_owned());
            }
            let c = self.peek()?;
            self.i += 1;
            match c {
                b'\\' if q == b'"' => {
                    let e = self.peek()?;
                    self.i += 1;
                    let c = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'e' => '\u{1b}',
                        b'"' | b'\\' => e as char,
                        b'u' => self.hex(4)?,
                        b'U' => self.hex(8)?,
                        b'x' => self.hex(2)?,
                        // A backslash ending a line of a multi-line string skips the whitespace up to the next token.
                        b' ' | b'\t' | b'\n' | b'\r' if multi => {
                            while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                                self.i += 1;
                            }
                            continue;
                        }
                        _ => return None,
                    };
                    out.extend(c.encode_utf8(&mut [0; 4]).bytes());
                }
                b'\n' if !multi => return None,
                c => out.push(c),
            }
        }
    }
    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        Some(match self.peek()? {
            b'"' | b'\'' => Value::Str(self.string(true)?),
            b'[' => {
                self.i += 1;
                let mut items = vec![];
                loop {
                    self.blank(true);
                    if self.eat(b']') {
                        break;
                    }
                    items.push(self.value(depth + 1)?);
                    self.blank(true);
                    if !self.eat(b',') {
                        self.eat(b']').then_some(())?;
                        break;
                    }
                }
                Value::Array(items)
            }
            b'{' => {
                self.i += 1;
                let mut table = Table::new();
                loop {
                    // TOML 1.1 allows newlines, comments and a trailing comma inside inline tables.
                    self.blank(true);
                    if self.eat(b'}') {
                        break;
                    }
                    let key = self.key()?;
                    self.blank(false);
                    self.eat(b'=').then_some(())?;
                    self.blank(false);
                    let value = self.value(depth + 1)?;
                    self.count()?;
                    let (last, parent) = key.split_last()?;
                    descend(&mut table, parent)?.insert(last.clone(), value);
                    self.blank(true);
                    if !self.eat(b',') {
                        self.eat(b'}').then_some(())?;
                        break;
                    }
                }
                Value::Table(table)
            }
            _ => {
                let start = self.i;
                while !matches!(
                    self.peek(),
                    Some(b',' | b']' | b'}' | b'#' | b' ' | b'\t' | b'\n' | b'\r') | None
                ) {
                    self.i += 1;
                }
                (self.i > start).then_some(())?;
                Value::Other
            }
        })
    }
}
/// The table at `path`, created on the way; the last element of an array of tables counts as a table.
fn descend<'t>(mut table: &'t mut Table, path: &[String]) -> Option<&'t mut Table> {
    for key in path {
        table = match table
            .entry(key.clone())
            .or_insert_with(|| Value::Table(Table::new()))
        {
            Value::Table(t) => t,
            Value::Array(items) => match items.last_mut()? {
                Value::Table(t) => t,
                _ => return None,
            },
            _ => return None,
        };
    }
    Some(table)
}
#[cfg(test)]
mod tests {
    use super::*;
    const MANIFEST: &str = r##"# comment
[package]
name = "app" # trailing
description = """first
line "quoted" \
   continued ""
"""
weird."quoted key".x = 'lit\'
version.workspace = true
[ target.'cfg(unix)'.dependencies ]
libc = { version = "0.2", features = ["a"] }
serde = {
  version = "1", # comment
  features = ["derive",
    "rc"],
}
[features]
default = [
  "std", # comment
  "dep:x",
]
n = 1.5
d = 1979-05-27T07:32:00Z
emoji = "é😀é\U0001F600 \"x\" \\"
[[bin]]
name = "one"
[[bin]]
name = "two"
[bin.extra]
x = 1
[profile.release.package.foo]
opt-level = 3
"##;
    #[test]
    fn reads_realistic_manifest() {
        let doc = parse(MANIFEST);
        let package = doc.get("package").unwrap();
        assert_eq!(package.get("name").unwrap().as_str(), Some("app"));
        assert_eq!(
            package.get("description").unwrap().as_str(),
            Some("first\nline \"quoted\" continued \"\"\n")
        );
        assert_eq!(
            package.at(&["weird", "quoted key", "x"]).unwrap().as_str(),
            Some("lit\\")
        );
        let deps = doc.at(&["target", "cfg(unix)", "dependencies"]).unwrap();
        assert_eq!(deps.keys().collect::<Vec<_>>(), ["libc", "serde"]);
        let serde = deps.get("serde").unwrap();
        assert_eq!(serde.at(&["version"]).unwrap().as_str(), Some("1"));
        assert_eq!(
            serde.get("features").unwrap().strings().collect::<Vec<_>>(),
            ["derive", "rc"]
        );
        assert_eq!(
            doc.at(&["features", "default"])
                .unwrap()
                .strings()
                .collect::<Vec<_>>(),
            ["std", "dep:x"]
        );
        assert_eq!(doc.at(&["features", "n"]), Some(&Value::Other));
        assert_eq!(
            doc.at(&["features", "emoji"]).unwrap().as_str(),
            Some("é😀é😀 \"x\" \\")
        );
        assert_eq!(doc.names_at(&["bin"]), ["one", "two"]);
        assert!(doc.at(&["bin"]).unwrap().items()[1].get("extra").is_some());
        assert!(
            doc.at(&["profile", "release", "package", "foo", "opt-level"])
                .is_some()
        );
    }
    #[test]
    fn errors_keep_what_came_before() {
        let doc = parse("[a]\nx = \"1\"\nbad line here\n[b]\ny = \"2\"\n");
        assert_eq!(doc.at(&["a", "x"]).unwrap().as_str(), Some("1"));
        assert!(doc.get("b").is_none());
        // A string ending in a backslash at the end of the file used to index out of range.
        let doc = parse("[package]\nname = \"app\"\ndescription = \"abc\\");
        assert_eq!(doc.at(&["package", "name"]).unwrap().as_str(), Some("app"));
        assert!(doc.at(&["package", "description"]).is_none());
        for text in [
            "a = \"\"\"abc\\",
            "a = '''x",
            "a = \"\\u12",
            "a = \"\\",
            "[[",
            "[a",
            "a = {",
            "a = [\"x\"",
        ] {
            parse(text);
        }
        let deep = format!("a = {}", "[".repeat(500));
        assert!(parse(&deep).get("a").is_none());
        parse(&format!("a = {}", "{a=".repeat(500)));
    }
    #[test]
    fn truncation_at_every_offset_never_panics() {
        for n in 0..=MANIFEST.len() {
            let cut = String::from_utf8_lossy(&MANIFEST.as_bytes()[..n]).into_owned();
            parse(&cut);
        }
    }
}
