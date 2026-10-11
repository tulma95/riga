//! The built-in value sources a `.riga` spec can name (`<git-branch>`, `<git-ref>`, ...): one
//! token table, and exhaustive dispatch and path rules. Sources read local files and never depend
//! on the spec model.
use crate::{git, names::Ref};
use std::path::Path;
/// Everything a source may consult; all of it comes from the request or the engine, never from the
/// daemon's own environment, so sources are injectable and testable.
pub struct Env<'a> {
    pub cwd: &'a Path,
    pub home: &'a Path,
    /// The shell's PATH.
    pub path: &'a str,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Git(git::Kind),
}
/// Spec value tokens (`<token>`) and their sources.
const TOKENS: &[(&str, Source)] = &[
    ("git-branch", Source::Git(git::Kind::Branch)),
    ("git-ref", Source::Git(git::Kind::Ref)),
    ("git-remote", Source::Git(git::Kind::Remote)),
];
impl Source {
    pub fn parse(token: &str) -> Option<Source> {
        TOKENS.iter().find(|(t, _)| *t == token).map(|&(_, s)| s)
    }
    /// Candidates sorted by name and capped; empty when the underlying data is missing.
    pub fn complete(self, env: &Env) -> Vec<Ref> {
        match self {
            Source::Git(k) => git::complete(env, k),
        }
    }
    /// The suggestion kind shown for its candidates.
    pub fn suggestion_kind(self) -> &'static str {
        match self {
            Source::Git(_) => "ref",
        }
    }
    /// Whether file system paths are offered next to the candidates. Positionals that may also name a
    /// file (`git checkout -- f`) say yes; flag values and names that are never paths say no.
    pub fn offers_paths(self, is_flag: bool) -> bool {
        match self {
            Source::Git(_) => !is_flag,
        }
    }
}
/// Runs `f` with an environment for tests.
#[cfg(test)]
pub fn with_env<R>(cwd: &Path, home: &Path, path: &str, f: impl FnOnce(&Env) -> R) -> R {
    f(&Env { cwd, home, path })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokens_are_unique_and_parse_back() {
        for (i, (token, source)) in TOKENS.iter().enumerate() {
            assert_eq!(Source::parse(token), Some(*source));
            assert!(
                TOKENS[..i].iter().all(|(t, s)| t != token && s != source),
                "{token}"
            );
        }
        assert_eq!(Source::parse("branch"), None);
    }
    #[test]
    fn missing_data_gives_only_built_in_candidates() {
        let nowhere = Path::new("/nonexistent-riga");
        for (token, source) in TOKENS {
            let found = with_env(nowhere, nowhere, "", |env| source.complete(env));
            assert!(found.is_empty(), "{token}");
        }
    }
}
