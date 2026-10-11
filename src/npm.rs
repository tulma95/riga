//! npm script, dependency and workspace names read from the nearest `package.json`, and local binaries
//! listed from the nearest `node_modules/.bin`; never runs npm or project code.
use crate::{
    fsio::{self, read_text},
    json::Json,
    names::{Names, Ref},
    sources::Env,
};
use std::path::Path;
const MAX_BYTES: u64 = 1 << 20;
const MAX_NAMES: usize = 5_000;
const MAX_DEPTH: usize = 64;
const MAX_WORKSPACES: usize = 500;
const DEPENDENCY_FIELDS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Script,
    Dependency,
    /// Executables in the nearest `node_modules/.bin`.
    Bin,
    /// Directories matched by the root package.json `workspaces` patterns.
    Workspace,
}
fn package_json(dir: &Path) -> Option<String> {
    read_text(&dir.join("package.json"), MAX_BYTES)
}
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    match kind {
        Kind::Bin => bins(env.cwd),
        // Inside a workspace package, npm finds the root that declares it.
        Kind::Workspace => env
            .cwd
            .ancestors()
            .take(MAX_DEPTH)
            .filter_map(|dir| Some(workspaces(dir, &package_json(dir)?)))
            .find(|found| !found.is_empty())
            .unwrap_or_default(),
        // npm uses the nearest ancestor with a package.json as the project.
        Kind::Script => env
            .cwd
            .ancestors()
            .find_map(package_json)
            .map(|text| members(&text, &["scripts"], false))
            .unwrap_or_default(),
        Kind::Dependency => env
            .cwd
            .ancestors()
            .find_map(package_json)
            .map(|text| members(&text, &DEPENDENCY_FIELDS, true))
            .unwrap_or_default(),
    }
}
/// Member names of the given top-level objects, described by their string values (prefixed with the
/// field for dependencies). A malformed file still yields the names read before the error.
fn members(text: &str, fields: &[&str], with_field: bool) -> Vec<Ref> {
    let mut out = Names::new(MAX_NAMES);
    let _ = Json::new(text).object(&mut |p, field| {
        if !fields.contains(&field.as_str()) {
            return p.skip();
        }
        p.object(&mut |p, name| {
            let value = p.string_or_skip()?.unwrap_or_default();
            if with_field {
                out.push(&name, &format!("{field} {value}"));
            } else {
                out.push(&name, &value);
            }
            Some(())
        })
    });
    out.finish()
}
fn bins(cwd: &Path) -> Vec<Ref> {
    let Some(entries) = cwd
        .ancestors()
        .find_map(|dir| fsio::list_dir(&dir.join("node_modules/.bin"), fsio::MAX_DIR_ENTRIES))
    else {
        return vec![];
    };
    let mut out = Names::new(MAX_NAMES);
    for e in entries.iter().filter(|e| !e.name.starts_with('.')) {
        out.push(&e.name, "node_modules/.bin");
    }
    out.finish()
}
/// Workspace paths relative to the project root; `--workspace` accepts paths as well as package names.
/// ponytail: only literal paths and a single trailing `*` segment (`packages/*`) are expanded; `**`,
/// negations and other globs are skipped. Names come from directories, not each workspace's package.json.
fn workspaces(root: &Path, text: &str) -> Vec<Ref> {
    let mut patterns = vec![];
    let strings = |p: &mut Json, out: &mut Vec<String>| {
        p.array(&mut |p| {
            out.extend(p.string_or_skip()?);
            Some(())
        })
    };
    let _ = Json::new(text).object(&mut |p, field| {
        if field != "workspaces" {
            return p.skip();
        }
        if p.at_object() {
            // Yarn-style `{"packages": [...]}`.
            p.object(&mut |p, key| {
                if key == "packages" {
                    strings(p, &mut patterns)
                } else {
                    p.skip()
                }
            })
        } else {
            strings(p, &mut patterns)
        }
    });
    let mut out = Names::new(MAX_WORKSPACES);
    let mut push = |name: String| {
        if root.join(&name).join("package.json").is_file() {
            out.push(&name, "Workspace");
        }
    };
    for pattern in patterns.iter().take(MAX_WORKSPACES) {
        let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
        if pattern.split('/').any(|part| part == "..") || pattern.starts_with('/') {
            continue;
        }
        match pattern.strip_suffix("/*") {
            Some(parent) if !parent.contains(['*', '?', '[', '{', '!']) => {
                let entries = fsio::list_dir(&root.join(parent), fsio::MAX_DIR_ENTRIES);
                for e in entries
                    .iter()
                    .flatten()
                    .filter(|e| !e.name.starts_with('.'))
                {
                    push(format!("{parent}/{}", e.name));
                }
            }
            None if !pattern.contains(['*', '?', '[', '{', '!']) => push(pattern.into()),
            _ => {}
        }
    }
    out.finish()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::with_env;
    use std::fs;
    fn complete_at(cwd: &Path, kind: Kind) -> Vec<Ref> {
        with_env(cwd, cwd, "", |env| complete(env, kind))
    }
    fn names(cwd: &Path, kind: Kind) -> Vec<String> {
        complete_at(cwd, kind).into_iter().map(|r| r.name).collect()
    }
    #[test]
    fn reads_scripts_and_dependencies_from_nearest_package_json() {
        let root = std::env::temp_dir().join(format!("riga-npm-{}", std::process::id()));
        fs::create_dir_all(root.join("app/src/deep")).unwrap();
        fs::write(
            root.join("app/package.json"),
            r#"{"name": "app", "files": ["a", {"b": [1, true, null]}], "x": -1.5e3,
              "scripts": {"build": "tsc -p .", "test:unit": "node --test", "wé": "x", "bad\u202e": "x",
                          "ctl": "echo \u001b[31m"},
              "dependencies": {"react": "^19.0.0", "left-pad": "1.3.0"},
              "devDependencies": {"typescript": "~5.6", "react": "^19.0.0"},
              "peerDependenciesMeta": {"ignored": {"optional": true}}}"#,
        )
        .unwrap();
        let deep = root.join("app/src/deep");
        assert_eq!(
            names(&deep, Kind::Script),
            ["build", "ctl", "test:unit", "wé"]
        );
        let scripts = complete_at(&deep, Kind::Script);
        assert_eq!(scripts[0].description, "tsc -p .");
        assert_eq!(scripts[1].description, "");
        assert_eq!(
            names(&deep, Kind::Dependency),
            ["left-pad", "react", "typescript"]
        );
        assert_eq!(
            complete_at(&deep, Kind::Dependency)[2].description,
            "devDependencies ~5.6"
        );
        // Truncated or malformed input keeps what was read before the error.
        fs::write(
            root.join("app/package.json"),
            r#"{"scripts": {"a": "1", "b": "#,
        )
        .unwrap();
        assert_eq!(names(&deep, Kind::Script), ["a"]);
        assert!(complete_at(Path::new("/"), Kind::Script).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn lists_local_bins_and_workspaces() {
        let root = std::env::temp_dir().join(format!("riga-npm-ws-{}", std::process::id()));
        for dir in [
            "node_modules/.bin",
            "packages/b",
            "packages/a",
            "packages/no-manifest",
            "tools/cli/src",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (path, text) in [
            ("node_modules/.bin/tsc", ""),
            ("node_modules/.bin/eslint", ""),
            ("node_modules/.bin/.hidden", ""),
            ("node_modules/.bin/bad\u{202e}", ""),
            ("packages/a/package.json", "{}"),
            ("packages/b/package.json", "{}"),
            ("tools/cli/package.json", "{}"),
            (
                "package.json",
                r#"{"name": "mono", "workspaces": ["packages/*", "./tools/cli/", "../x", "**/deep", "missing"]}"#,
            ),
        ] {
            fs::write(root.join(path), text).unwrap();
        }
        let deep = root.join("tools/cli/src");
        assert_eq!(names(&deep, Kind::Bin), ["eslint", "tsc"]);
        // The nearest package.json (tools/cli) has no workspaces; the root does.
        assert_eq!(
            names(&deep, Kind::Workspace),
            ["packages/a", "packages/b", "tools/cli"]
        );
        fs::write(
            root.join("package.json"),
            r#"{"workspaces": {"packages": ["tools/*"], "nohoist": ["x"]}}"#,
        )
        .unwrap();
        assert_eq!(names(&root, Kind::Workspace), ["tools/cli"]);
        assert!(complete_at(Path::new("/"), Kind::Bin).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fifo_package_json_is_ignored_and_caps_follow_sorting() {
        let root = std::env::temp_dir().join(format!("riga-npm-fifo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(root.join("package.json"))
                .status()
                .unwrap()
                .success()
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = root.clone();
        std::thread::spawn(move || tx.send(names(&dir, Kind::Script)));
        assert!(
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("hung")
                .is_empty()
        );
        // More bins than the cap: the alphabetical first ones survive, not an arbitrary subset.
        for i in (0..MAX_NAMES + 10).rev() {
            fs::write(root.join(format!("node_modules/.bin/b{i:05}")), "").unwrap();
        }
        let bins = names(&root, Kind::Bin);
        assert_eq!(bins.len(), MAX_NAMES);
        assert_eq!(
            (bins[0].as_str(), bins[MAX_NAMES - 1].as_str()),
            ("b00000", "b04999")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
