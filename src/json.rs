//! Bounded pull-style JSON reader for `package.json` and VS Code storage files. Callers walk the
//! structure and keep only what they need; the reader never builds a tree. Every method returns
//! `None` on malformed input, and callers keep what they collected before that point.
const MAX_DEPTH: usize = 64;
pub struct Json<'a> {
    s: &'a [u8],
    i: usize,
}
impl<'a> Json<'a> {
    pub fn new(text: &'a str) -> Self {
        Json {
            s: text.as_bytes(),
            i: 0,
        }
    }
    fn ws(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        (self.s.get(self.i) == Some(&c)).then(|| self.i += 1)
    }
    /// Whether the next value is a string.
    pub fn at_string(&mut self) -> bool {
        self.ws();
        self.s.get(self.i) == Some(&b'"')
    }
    /// Whether the next value is an object.
    pub fn at_object(&mut self) -> bool {
        self.ws();
        self.s.get(self.i) == Some(&b'{')
    }
    fn hex4(&mut self) -> Option<u32> {
        let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(hex, 16).ok()
    }
    pub fn string(&mut self) -> Option<String> {
        self.eat(b'"')?;
        let mut out = vec![];
        loop {
            match *self.s.get(self.i)? {
                b'"' => {
                    self.i += 1;
                    return Some(String::from_utf8_lossy(&out).into_owned());
                }
                b'\\' => {
                    let escape = *self.s.get(self.i + 1)?;
                    self.i += 2;
                    let c = match escape {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'"' | b'\\' | b'/' => escape as char,
                        b'u' => {
                            let high = self.hex4()?;
                            if (0xd800..0xdc00).contains(&high)
                                && self.s[self.i..].starts_with(b"\\u")
                            {
                                self.i += 2;
                                let low = self.hex4()?;
                                (0xdc00..0xe000)
                                    .contains(&low)
                                    .then(|| 0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00))
                                    .and_then(char::from_u32)
                            } else {
                                char::from_u32(high)
                            }
                            .unwrap_or('\u{fffd}')
                        }
                        _ => return None,
                    };
                    out.extend(c.encode_utf8(&mut [0; 4]).bytes());
                }
                c => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }
    /// The string at the cursor, or `None` after skipping a value of another type.
    pub fn string_or_skip(&mut self) -> Option<Option<String>> {
        if self.at_string() {
            self.string().map(Some)
        } else {
            self.skip().map(|()| None)
        }
    }
    /// Calls `member` with each key; it must consume the value (read or `skip` it).
    pub fn object(
        &mut self,
        member: &mut dyn FnMut(&mut Json<'a>, String) -> Option<()>,
    ) -> Option<()> {
        self.eat(b'{')?;
        if self.eat(b'}').is_some() {
            return Some(());
        }
        loop {
            let key = self.string()?;
            self.eat(b':')?;
            member(self, key)?;
            if self.eat(b',').is_none() {
                return self.eat(b'}');
            }
        }
    }
    /// Calls `item` for each element; it must consume the value.
    pub fn array(&mut self, item: &mut dyn FnMut(&mut Json<'a>) -> Option<()>) -> Option<()> {
        self.eat(b'[')?;
        if self.eat(b']').is_some() {
            return Some(());
        }
        loop {
            item(self)?;
            if self.eat(b',').is_none() {
                return self.eat(b']');
            }
        }
    }
    pub fn skip(&mut self) -> Option<()> {
        self.skip_at(0)
    }
    fn skip_at(&mut self, depth: usize) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match *self.s.get(self.i)? {
            b'"' => self.string().map(drop),
            b'{' => self.object(&mut |p, _| p.skip_at(depth + 1)),
            b'[' => self.array(&mut |p| p.skip_at(depth + 1)),
            _ => {
                // Numbers and literals are skipped unvalidated.
                let start = self.i;
                while !matches!(
                    self.s.get(self.i),
                    Some(b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') | None
                ) {
                    self.i += 1;
                }
                (self.i > start).then_some(())
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_escapes_and_surrogate_pairs() {
        let text = r#""Café 😀 \"q\" \\ \/ \ud800x \n""#;
        assert_eq!(
            Json::new(text).string().unwrap(),
            "Café \u{1f600} \"q\" \\ / \u{fffd}x \n"
        );
        assert!(Json::new(r#""bad \q""#).string().is_none());
        assert!(Json::new(r#""cut \u00"#).string().is_none());
        assert!(Json::new(r#""cut \"#).string().is_none());
    }
    #[test]
    fn walks_objects_and_stops_at_errors() {
        let mut keys = vec![];
        let text = r#"{"a": [1, {"x": null}, "s"], "b": -1.5e3, "c": {"d": true}, "e": "#;
        let result = Json::new(text).object(&mut |p, key| {
            keys.push(key);
            p.skip()
        });
        assert!(result.is_none());
        assert_eq!(keys, ["a", "b", "c", "e"]);
        let deep = "[".repeat(200);
        assert!(Json::new(&deep).skip().is_none());
    }
    #[test]
    fn truncation_never_panics() {
        let text = r#"{"k": ["éé😀", {"n": 1.5}], "s": "x\"y"}"#;
        for n in 0..=text.len() {
            let bytes = &text.as_bytes()[..n];
            let cut = String::from_utf8_lossy(bytes);
            let _ = Json::new(&cut).skip();
        }
    }
}
