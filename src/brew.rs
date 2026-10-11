//! Homebrew names read from directory listings under the prefix and the local API name cache; never runs brew.
use crate::{
    fsio::{self, read_text},
    names::{Names, Ref},
    sources::Env,
};
use std::path::{Path, PathBuf};
const MAX_NAMES: usize = 20_000;
const MAX_LIST_BYTES: u64 = 1 << 20;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Every formula and cask in the API cache, plus installed ones.
    Package,
    /// Every formula in the API cache, plus installed ones.
    Formula,
    Installed,
    InstalledFormula,
    Pinned,
    Tap,
}
/// Prefix of the `brew` on the shell's PATH, else the default Apple silicon and Intel prefixes.
fn prefix(path: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute() && dir.join("brew").is_file())
        .filter_map(|dir| dir.parent().map(Path::to_path_buf))
        .chain([PathBuf::from("/opt/homebrew"), PathBuf::from("/usr/local")])
        .find(|p| p.join("bin/brew").is_file())
}
fn dir_names(dir: &Path) -> Vec<String> {
    fsio::list_dir(dir, fsio::MAX_DIR_ENTRIES)
        .into_iter()
        .flatten()
        .map(|e| e.name)
        .filter(|n| !n.starts_with('.'))
        .collect()
}
/// One name per line, as in Homebrew's `api/formula_names.txt`.
fn list_names(file: &Path) -> Vec<String> {
    read_text(file, MAX_LIST_BYTES)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|n| !n.starts_with('.'))
        .map(String::from)
        .collect()
}
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let Some(prefix) = prefix(env.path) else {
        return vec![];
    };
    let api = env.home.join("Library/Caches/Homebrew/api");
    let mut sources: Vec<(Vec<String>, &str)> = vec![];
    match kind {
        Kind::Package | Kind::Formula | Kind::Installed | Kind::InstalledFormula => {
            sources.push((dir_names(&prefix.join("Cellar")), "Installed formula"));
            if matches!(kind, Kind::Package | Kind::Installed) {
                sources.push((dir_names(&prefix.join("Caskroom")), "Installed cask"));
            }
            if matches!(kind, Kind::Package | Kind::Formula) {
                sources.push((list_names(&api.join("formula_names.txt")), "Formula"));
            }
            if matches!(kind, Kind::Package) {
                sources.push((list_names(&api.join("cask_names.txt")), "Cask"));
            }
        }
        Kind::Pinned => sources.push((
            dir_names(&prefix.join("var/homebrew/pinned")),
            "Pinned formula",
        )),
        Kind::Tap => {
            // Intel installs keep the repository (and its taps) in prefix/Homebrew.
            for library in [prefix.join("Library"), prefix.join("Homebrew/Library")] {
                let taps = library.join("Taps");
                let mut names = vec![];
                for user in dir_names(&taps) {
                    for repo in dir_names(&taps.join(&user)) {
                        if let Some(repo) = repo.strip_prefix("homebrew-") {
                            names.push(format!("{user}/{repo}"));
                        }
                    }
                }
                sources.push((names, "Tap"));
            }
        }
    }
    let mut out = Names::new(MAX_NAMES);
    for (names, description) in sources {
        for name in names {
            out.push(&name, description);
        }
    }
    out.finish()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::with_env;
    use std::fs;
    fn refs(path: &str, home: &Path, kind: Kind) -> Vec<Ref> {
        with_env(home, home, path, |env| complete(env, kind))
    }
    fn names(path: &str, home: &Path, kind: Kind) -> Vec<String> {
        refs(path, home, kind).into_iter().map(|r| r.name).collect()
    }
    #[test]
    fn reads_installed_pinned_taps_and_api_names() {
        let root = std::env::temp_dir().join(format!("riga-brew-{}", std::process::id()));
        let prefix = root.join("brew");
        for dir in [
            "bin",
            "Cellar/wget/1.25",
            "Cellar/ffmpeg/8.0",
            "Cellar/.hidden",
            "Cellar/bad\u{202e}",
            "Caskroom/firefox",
            "var/homebrew/pinned/ffmpeg",
            "Library/Taps/homebrew/homebrew-core",
            "Library/Taps/me/homebrew-tools",
            "Library/Taps/me/not-a-tap",
        ] {
            fs::create_dir_all(prefix.join(dir)).unwrap();
        }
        fs::write(prefix.join("bin/brew"), "").unwrap();
        let api = root.join("home/Library/Caches/Homebrew/api");
        fs::create_dir_all(&api).unwrap();
        fs::write(api.join("formula_names.txt"), "a2ps\nwget\n\nevil\u{1b}x\n").unwrap();
        fs::write(api.join("cask_names.txt"), "zed\n").unwrap();
        let home = root.join("home");
        let path = format!("/nonexistent-riga:{}", prefix.join("bin").display());
        assert_eq!(
            names(&path, &home, Kind::Installed),
            ["ffmpeg", "firefox", "wget"]
        );
        assert_eq!(
            names(&path, &home, Kind::InstalledFormula),
            ["ffmpeg", "wget"]
        );
        assert_eq!(
            names(&path, &home, Kind::Package),
            ["a2ps", "ffmpeg", "firefox", "wget", "zed"]
        );
        assert_eq!(
            names(&path, &home, Kind::Formula),
            ["a2ps", "ffmpeg", "wget"]
        );
        let all = refs(&path, &home, Kind::Package);
        assert_eq!(all[0].description, "Formula");
        assert_eq!(all[1].description, "Installed formula");
        assert_eq!(all[2].description, "Installed cask");
        assert_eq!(names(&path, &home, Kind::Pinned), ["ffmpeg"]);
        assert_eq!(
            names(&path, &home, Kind::Tap),
            ["homebrew/core", "me/tools"]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
