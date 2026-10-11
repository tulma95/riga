//! VS Code extension IDs and profile names read from the user's VS Code data; never runs `code`.
use crate::{
    fsio::{self, read_text},
    json::Json,
    names::{Names, Ref},
    sources::Env,
};
use std::{collections::HashSet, path::Path};
const MAX_NAMES: usize = 5_000;
const MAX_BYTES: u64 = 1 << 20;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Extension,
    Profile,
}
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    match kind {
        Kind::Extension => extensions(&env.home.join(".vscode/extensions")),
        Kind::Profile => profiles(
            &read_text(
                &env.home
                    .join("Library/Application Support/Code/User/globalStorage/storage.json"),
                MAX_BYTES,
            )
            .unwrap_or_default(),
        ),
    }
}
/// Installed extensions are folders named `publisher.name-version[-platform]`.
fn extensions(dir: &Path) -> Vec<Ref> {
    let Some(entries) = fsio::list_dir(dir, fsio::MAX_DIR_ENTRIES) else {
        return vec![];
    };
    // Superseded versions stay on disk until VS Code cleans up; `.obsolete` lists them by folder.
    let mut obsolete = HashSet::new();
    let _ = Json::new(&read_text(&dir.join(".obsolete"), MAX_BYTES).unwrap_or_default()).object(
        &mut |p, folder| {
            obsolete.insert(folder);
            p.skip()
        },
    );
    let mut out = Names::new(MAX_NAMES);
    for entry in entries {
        if entry.name.starts_with('.') || obsolete.contains(&entry.name) {
            continue;
        }
        if let Some((id, _)) = split_version(&entry.name)
            && id.contains('.')
        {
            out.push(id, "Installed extension");
        }
    }
    out.finish()
}
/// Splits at the first `-` that starts a dotted numeric version: `a.b-c-1.2.3-darwin-arm64`.
fn split_version(folder: &str) -> Option<(&str, &str)> {
    folder.match_indices('-').find_map(|(i, _)| {
        let rest = &folder[i + 1..];
        let version = rest.split('-').next()?;
        (version.contains('.')
            && version.starts_with(|c: char| c.is_ascii_digit())
            && version.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .then(|| (&folder[..i], rest))
    })
}
/// The `name` of each object in the top-level `userDataProfiles` array.
fn profiles(text: &str) -> Vec<Ref> {
    let mut out = Names::new(MAX_NAMES);
    let _ = Json::new(text).object(&mut |p, key| {
        if key != "userDataProfiles" {
            return p.skip();
        }
        p.array(&mut |p| {
            if !p.at_object() {
                return p.skip();
            }
            p.object(&mut |p, key| {
                if key == "name" {
                    if let Some(name) = p.string_or_skip()? {
                        out.push(&name, "Profile");
                    }
                    Some(())
                } else {
                    p.skip()
                }
            })
        })
    });
    out.finish()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::with_env;
    use std::fs;
    fn names(home: &Path, kind: Kind) -> Vec<String> {
        with_env(home, home, "", |env| complete(env, kind))
            .into_iter()
            .map(|r| r.name)
            .collect()
    }
    #[test]
    fn reads_extensions_and_profiles() {
        let home = std::env::temp_dir().join(format!("riga-vscode-{}", std::process::id()));
        let ext = home.join(".vscode/extensions");
        for folder in [
            "astro-build.astro-vscode-2.16.18-darwin-arm64",
            "astro-build.astro-vscode-2.16.20-darwin-arm64",
            "dart-code.dart-code-3.140.0",
            "ms-python.python-2024.1.0",
            "old.gone-1.0.0",
            "no-version",
            ".hidden.ext-1.0.0",
        ] {
            fs::create_dir_all(ext.join(folder)).unwrap();
        }
        fs::write(ext.join(".obsolete"), r#"{"old.gone-1.0.0":true}"#).unwrap();
        assert_eq!(
            names(&home, Kind::Extension),
            [
                "astro-build.astro-vscode",
                "dart-code.dart-code",
                "ms-python.python"
            ]
        );
        assert!(names(&home, Kind::Profile).is_empty());
        let storage = home.join("Library/Application Support/Code/User/globalStorage");
        fs::create_dir_all(&storage).unwrap();
        fs::write(
            storage.join("storage.json"),
            r#"{"name": "not-a-profile", "userDataProfiles" : [
                {"location": "abc", "name": "Work \"A\"", "useDefaultFlags": {"name": "nested"}},
                {"name": "Caf\u00e9 \ud83d\ude00"},
                {"name":"Rust","icon":"x"}, {"location": "name", "name": "bad@RLO"}
              ], "after": {"name": "no"}}"#
                .replace("@RLO", "\u{202e}"),
        )
        .unwrap();
        assert_eq!(
            names(&home, Kind::Profile),
            ["Café \u{1f600}", "Rust", "Work \"A\""]
        );
        fs::remove_dir_all(home).unwrap();
    }
}
