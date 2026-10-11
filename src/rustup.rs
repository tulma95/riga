//! rustup toolchains, targets and components read from `~/.rustup`; never runs rustup.
use crate::{
    fsio::{self, read_text},
    names::{Names, Ref},
    sources::Env,
};
const MAX_TOOLCHAINS: usize = 200;
const MAX_NAMES: usize = 2_000;
const MAX_BYTES: u64 = 64 << 10;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Toolchain,
    Target,
    Component,
}
/// ponytail: always `$HOME/.rustup`; a custom `RUSTUP_HOME` in the shell is not seen (not in `State`).
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let dir = env.home.join(".rustup/toolchains");
    let toolchains: Vec<String> = fsio::list_dir(&dir, MAX_TOOLCHAINS)
        .into_iter()
        .flatten()
        .map(|e| e.name)
        .filter(|n| !n.starts_with('.'))
        .collect();
    let mut out = Names::new(MAX_NAMES);
    if kind == Kind::Toolchain {
        for t in &toolchains {
            out.push(t, "Installed toolchain");
        }
        for channel in ["stable", "beta", "nightly"] {
            out.push(channel, "Release channel");
        }
        return out.finish();
    }
    // ponytail: union over all installed toolchains, not just the active or --toolchain one.
    for t in &toolchains {
        let Some(text) = read_text(&dir.join(t).join("lib/rustlib/components"), MAX_BYTES) else {
            continue;
        };
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| !l.chars().any(crate::spec::deceptive))
            .collect();
        let targets: Vec<&str> = lines
            .iter()
            .filter_map(|l| l.strip_prefix("rust-std-"))
            .collect();
        if kind == Kind::Target {
            for target in targets {
                out.push(target, "Installed target");
            }
            continue;
        }
        // Lines are `<component>[-preview]-<target>`; the host is the target the toolchain name ends with.
        let Some(host) = targets.iter().find(|h| t.ends_with(&format!("-{h}"))) else {
            continue;
        };
        for l in &lines {
            if let Some(c) = l.strip_suffix(&format!("-{host}")) {
                out.push(c.trim_end_matches("-preview"), "Installed component");
            }
        }
    }
    out.finish()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::with_env;
    use std::fs;
    #[test]
    fn values_come_from_rustup_home() {
        let home = std::env::temp_dir().join(format!("riga-rustup-{}", std::process::id()));
        let lib = home.join(".rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib");
        fs::create_dir_all(&lib).unwrap();
        fs::create_dir_all(home.join(".rustup/toolchains/my-build")).unwrap();
        fs::write(
            lib.join("components"),
            "cargo-aarch64-apple-darwin\nclippy-preview-aarch64-apple-darwin\n\
             rust-std-aarch64-apple-darwin\nrust-std-wasm32-unknown-unknown\n",
        )
        .unwrap();
        let names = |k| -> Vec<String> {
            with_env(&home, &home, "", |env| complete(env, k))
                .into_iter()
                .map(|r| r.name)
                .collect()
        };
        assert_eq!(
            names(Kind::Toolchain),
            [
                "beta",
                "my-build",
                "nightly",
                "stable",
                "stable-aarch64-apple-darwin"
            ]
        );
        assert_eq!(
            names(Kind::Target),
            ["aarch64-apple-darwin", "wasm32-unknown-unknown"]
        );
        assert_eq!(names(Kind::Component), ["cargo", "clippy", "rust-std"]);
        fs::remove_dir_all(&home).unwrap();
        // Missing rustup home still offers channels.
        assert_eq!(names(Kind::Toolchain), ["beta", "nightly", "stable"]);
        assert!(names(Kind::Target).is_empty());
    }
}
