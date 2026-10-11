//! Small version-1 line protocol. Only %, TAB, LF and CR are percent escaped.
use std::path::PathBuf;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub session: String,
    pub pane: String,
    pub revision: u64,
    pub cursor: usize,
    pub cwd: PathBuf,
    pub buffer: String,
    pub path: String,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    State(State),
    End(String, u64),
    Action(String, u64, String),
    /// The connection's key bindings in zsh `bindkey` notation: next, prev, accept, dismiss.
    Keys([String; 4]),
}
pub fn encode(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\t', "%09")
        .replace('\n', "%0A")
        .replace('\r', "%0D")
}
pub fn decode(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let a = chars.next()?;
        let b = chars.next()?;
        out.push(match (a, b) {
            ('2', '5') => '%',
            ('0', '9') => '\t',
            ('0', 'A') => '\n',
            ('0', 'D') => '\r',
            _ => return None,
        });
    }
    Some(out)
}
pub fn parse(line: &str) -> Option<Message> {
    let f: Vec<_> = line.split('\t').collect();
    let valid_id = |s: &str| {
        !s.is_empty()
            && s.len() < 256
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
    };
    match f.as_slice() {
        ["S", id, pane, rev, cursor, cwd, buffer, path] if valid_id(id) && valid_id(pane) => {
            let buffer = decode(buffer)?;
            let cursor = cursor.parse().ok()?;
            if cursor > buffer.chars().count() {
                return None;
            }
            Some(Message::State(State {
                session: (*id).into(),
                pane: (*pane).into(),
                revision: rev.parse().ok()?,
                cursor,
                cwd: decode(cwd)?.into(),
                buffer,
                path: decode(path)?,
            }))
        }
        ["E", id, rev] if valid_id(id) => Some(Message::End((*id).into(), rev.parse().ok()?)),
        ["A", id, rev, action]
            if valid_id(id) && ["next", "prev", "accept", "dismiss"].contains(action) =>
        {
            Some(Message::Action(
                (*id).into(),
                rev.parse().ok()?,
                (*action).into(),
            ))
        }
        ["K", next, prev, accept, dismiss] => {
            let key = |k: &str| decode(k).filter(|k| k.chars().count() <= 16);
            Some(Message::Keys([
                key(next)?,
                key(prev)?,
                key(accept)?,
                key(dismiss)?,
            ]))
        }
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaping_roundtrip() {
        let s = "🌍\t\n%0A\r'";
        assert_eq!(decode(&encode(s)).as_deref(), Some(s));
        assert!(decode("%zz").is_none());
    }
    #[test]
    fn cursor_is_characters() {
        assert!(parse("S\ts\tp\t1\t2\t/tmp\té🌍\t/bin").is_some());
        assert!(parse("S\ts\tp\t1\t3\t/tmp\té🌍\t/bin").is_none());
    }
}
