//! AWS CLI profile and SSO session names read from `~/.aws/config` and `~/.aws/credentials`; never
//! runs aws (it would need credentials and the network).
use crate::{
    fsio::read_text,
    names::{Names, Ref},
    sources::Env,
};
const MAX_BYTES: u64 = 1 << 20;
const MAX_NAMES: usize = 5_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Profile,
    SsoSession,
}
/// ponytail: default paths only; `$AWS_CONFIG_FILE` and `$AWS_SHARED_CREDENTIALS_FILE` live in the
/// shell environment, which the daemon does not see.
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let mut names = Names::new(MAX_NAMES);
    let read = |file: &str| read_text(&env.home.join(".aws").join(file), MAX_BYTES);
    for section in read("config").iter().flat_map(|t| sections(t)) {
        let (prefix, name) = match section.split_once(char::is_whitespace) {
            Some((prefix, name)) => (prefix, name.trim()),
            None => ("", section),
        };
        match (kind, prefix, name) {
            (Kind::Profile, "", "default") | (Kind::Profile, "profile", _) => {
                names.push(name, "Profile in ~/.aws/config")
            }
            (Kind::SsoSession, "sso-session", _) => {
                names.push(name, "SSO session in ~/.aws/config")
            }
            _ => {}
        }
    }
    if kind == Kind::Profile {
        for name in read("credentials").iter().flat_map(|t| sections(t)) {
            names.push(name, "Profile in ~/.aws/credentials");
        }
    }
    names.finish()
}
/// The trimmed text of each `[section]` header line in an INI file.
fn sections(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter_map(|line| {
        let inner = line.trim().strip_prefix('[')?.strip_suffix(']')?;
        Some(inner.trim())
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::with_env;
    use std::fs;
    #[test]
    fn profiles_come_from_config_and_credentials() {
        let home = std::env::temp_dir().join(format!("riga-aws-{}", std::process::id()));
        fs::create_dir_all(home.join(".aws")).unwrap();
        fs::write(
            home.join(".aws/config"),
            "[default]\nregion = eu-north-1\n# [profile commented]\n[profile  dev ]\n\
             [profile prod]\nsso_session = corp\n[sso-session corp]\n[services local]\n\
             [profile bad\u{202e}x]\n[plain]\n",
        )
        .unwrap();
        fs::write(
            home.join(".aws/credentials"),
            "[default]\naws_access_key_id = x\n[ci]\n; [skip]\n",
        )
        .unwrap();
        let got = |kind| {
            with_env(&home, &home, "", |env| complete(env, kind))
                .into_iter()
                .map(|r| (r.name, r.description))
                .collect::<Vec<_>>()
        };
        let profiles = got(Kind::Profile);
        let names: Vec<_> = profiles.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["ci", "default", "dev", "prod"]);
        assert_eq!(profiles[1].1, "Profile in ~/.aws/config");
        assert_eq!(profiles[0].1, "Profile in ~/.aws/credentials");
        assert_eq!(got(Kind::SsoSession)[0].0, "corp");
        assert_eq!(got(Kind::SsoSession).len(), 1);
        fs::remove_dir_all(&home).unwrap();
        assert!(got(Kind::Profile).is_empty());
    }
}
