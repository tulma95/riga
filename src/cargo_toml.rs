//! Cargo package, target, feature, dependency and profile names read from `Cargo.toml`, `Cargo.lock`
//! and `~/.cargo/.crates.toml`. Never runs cargo, build scripts or any project code.
use crate::{
    fsio::{self, read_text},
    names::{Names, Ref},
    sources::Env,
    toml::{self, Value},
};
use std::path::{Path, PathBuf};
const MAX_BYTES: u64 = 1 << 20;
const MAX_NAMES: usize = 5_000;
const MAX_MEMBERS: usize = 500;
const DEPENDENCY_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Packages of the workspace (or the single package).
    Package,
    /// Workspace packages plus every package in `Cargo.lock`.
    LockPackage,
    Bin,
    Example,
    Test,
    Bench,
    Feature,
    Dependency,
    Profile,
    /// Packages recorded by `cargo install`.
    Installed,
}
/// A target table (`[[bin]]`) and the directory cargo discovers such targets in.
#[derive(Clone, Copy)]
struct Target {
    table: &'static str,
    dir: &'static str,
}
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let mut names = Names::new(MAX_NAMES);
    match kind {
        // ponytail: always `~/.cargo`; a custom `CARGO_HOME` or `--root` is not followed.
        Kind::Installed => installed(&env.home.join(".cargo"), &mut names),
        Kind::Bin => targets(
            env.cwd,
            Target {
                table: "bin",
                dir: "src/bin",
            },
            &mut names,
        ),
        Kind::Example => targets(
            env.cwd,
            Target {
                table: "example",
                dir: "examples",
            },
            &mut names,
        ),
        Kind::Test => targets(
            env.cwd,
            Target {
                table: "test",
                dir: "tests",
            },
            &mut names,
        ),
        Kind::Bench => targets(
            env.cwd,
            Target {
                table: "bench",
                dir: "benches",
            },
            &mut names,
        ),
        Kind::Package | Kind::LockPackage => {
            packages(env.cwd, kind == Kind::LockPackage, &mut names)
        }
        Kind::Feature => {
            if let Some(project) = Project::find(env.cwd) {
                for m in project.local() {
                    for (name, list) in m.doc.get("features").into_iter().flat_map(Value::entries) {
                        names.push(name, &list.strings().collect::<Vec<_>>().join(" "));
                    }
                }
            }
        }
        Kind::Dependency => {
            if let Some(project) = Project::find(env.cwd) {
                for m in project.local() {
                    dependencies(&m.doc, &mut names);
                }
            }
        }
        Kind::Profile => {
            for name in ["dev", "release", "test", "bench"] {
                names.push(name, "Built-in profile");
            }
            if let Some(project) = Project::find(env.cwd) {
                for name in project
                    .root()
                    .doc
                    .get("profile")
                    .into_iter()
                    .flat_map(Value::keys)
                {
                    names.push(name, "Custom profile");
                }
            }
        }
    }
    names.finish()
}
fn packages(cwd: &Path, locked: bool, names: &mut Names) {
    let Some(project) = Project::find(cwd) else {
        return;
    };
    let root = project.root();
    for m in std::iter::once(root).chain(&project.members) {
        if let Some(name) = m.package() {
            names.push(name, "Workspace package");
        }
    }
    // The current package even if the workspace member list missed it.
    if let Some(name) = project.nearest.package() {
        names.push(name, "Package");
    }
    if locked && let Some(lock) = load(&root.dir.join("Cargo.lock")) {
        for name in lock.names_at(&["package"]) {
            names.push(name, "Locked package");
        }
    }
}
/// `[[bin]]`-style targets of the current package (every member at a workspace root), plus the
/// packages' auto-discovered ones. ponytail: auto-discovery ignores autobins/autoexamples = false and
/// custom `path`s.
fn targets(cwd: &Path, target: Target, names: &mut Names) {
    let Some(project) = Project::find(cwd) else {
        return;
    };
    for m in project.local() {
        let description = match m.package() {
            Some(package) => format!("{} of {package}", capitalise(target.table)),
            None => capitalise(target.table),
        };
        for name in m.doc.names_at(&[target.table]) {
            names.push(name, &description);
        }
        if target.table == "bin"
            && m.dir.join("src/main.rs").is_file()
            && let Some(name) = m.package()
        {
            names.push(name, &description);
        }
        for e in fsio::list_dir(&m.dir.join(target.dir), fsio::MAX_DIR_ENTRIES)
            .into_iter()
            .flatten()
        {
            match e.name.strip_suffix(".rs") {
                Some(stem) if m.dir.join(target.dir).join(&e.name).is_file() => {
                    names.push(stem, &description)
                }
                _ if m
                    .dir
                    .join(target.dir)
                    .join(&e.name)
                    .join("main.rs")
                    .is_file() =>
                {
                    names.push(&e.name, &description)
                }
                _ => {}
            }
        }
    }
}
fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
/// `[dependencies]`, `[dev-dependencies]` and `[build-dependencies]`, also under `[target.<cfg>]`;
/// `[dependencies.serde]` and `serde.workspace = true` give the same name as `serde = "1"`.
fn dependencies(doc: &Value, names: &mut Names) {
    let targets = doc
        .get("target")
        .into_iter()
        .flat_map(Value::entries)
        .map(|(_, t)| t);
    for scope in std::iter::once(doc).chain(targets) {
        for section in DEPENDENCY_TABLES {
            for name in scope.get(section).into_iter().flat_map(Value::keys) {
                names.push(name, &capitalise(section));
            }
        }
    }
}
fn installed(cargo_home: &Path, names: &mut Names) {
    let doc = load(&cargo_home.join(".crates.toml")).unwrap_or(Value::Other);
    // Keys look like `"ripgrep 14.1.0 (registry+https://...)" = ["rg"]`.
    for (key, bins) in doc.get("v1").into_iter().flat_map(Value::entries) {
        let mut words = key.split(' ');
        let name = words.next().unwrap_or_default();
        let version = words.next().unwrap_or_default();
        names.push(
            name,
            &format!("{version} {}", bins.strings().collect::<Vec<_>>().join(" ")),
        );
    }
}
fn load(path: &Path) -> Option<Value> {
    Some(toml::parse(&read_text(path, MAX_BYTES)?))
}
struct Manifest {
    dir: PathBuf,
    doc: Value,
}
impl Manifest {
    fn load(dir: &Path) -> Option<Manifest> {
        Some(Manifest {
            doc: load(&dir.join("Cargo.toml"))?,
            dir: dir.into(),
        })
    }
    fn package(&self) -> Option<&str> {
        self.doc.at(&["package", "name"])?.as_str()
    }
    fn is_workspace(&self) -> bool {
        self.doc.get("workspace").is_some()
    }
    fn members(&self) -> Vec<Manifest> {
        let mut out = vec![];
        let patterns = self
            .doc
            .at(&["workspace", "members"])
            .into_iter()
            .flat_map(Value::strings);
        for member in patterns {
            // ponytail: only literal paths and a final `/*` glob; `exclude` is ignored.
            let dirs: Vec<PathBuf> = match member.strip_suffix("/*") {
                Some(parent) if !parent.contains(['*', '?', '[']) => {
                    fsio::list_dir(&self.dir.join(parent), fsio::MAX_DIR_ENTRIES)
                        .into_iter()
                        .flatten()
                        .map(|e| self.dir.join(parent).join(e.name))
                        .collect()
                }
                _ if !member.contains(['*', '?', '[']) => vec![self.dir.join(member)],
                _ => vec![],
            };
            for dir in dirs {
                if out.len() >= MAX_MEMBERS {
                    return out;
                }
                out.extend(Manifest::load(&dir));
            }
        }
        out
    }
}
/// The manifest nearest to the working directory and the workspace around it.
struct Project {
    nearest: Manifest,
    /// The workspace root when it is not the nearest manifest itself.
    workspace: Option<Manifest>,
    members: Vec<Manifest>,
}
impl Project {
    fn find(cwd: &Path) -> Option<Project> {
        let nearest = cwd.ancestors().find_map(Manifest::load)?;
        // Cargo looks further up for the workspace root when the nearest manifest is not one.
        let workspace = if nearest.is_workspace() {
            None
        } else {
            nearest
                .dir
                .parent()
                .into_iter()
                .flat_map(Path::ancestors)
                .find_map(|dir| Manifest::load(dir).filter(Manifest::is_workspace))
        };
        let members = workspace.as_ref().unwrap_or(&nearest).members();
        Some(Project {
            nearest,
            workspace,
            members,
        })
    }
    fn root(&self) -> &Manifest {
        self.workspace.as_ref().unwrap_or(&self.nearest)
    }
    /// Target, feature and dependency names come from the current package, or every member at the root.
    fn local(&self) -> Vec<&Manifest> {
        if self.nearest.is_workspace() {
            std::iter::once(&self.nearest)
                .chain(&self.members)
                .collect()
        } else {
            vec![&self.nearest]
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::with_env;
    use std::fs;
    fn refs(cwd: &Path, kind: Kind) -> Vec<Ref> {
        with_env(cwd, cwd, "", |env| complete(env, kind))
    }
    fn names(cwd: &Path, kind: Kind) -> Vec<String> {
        refs(cwd, kind).into_iter().map(|r| r.name).collect()
    }
    #[test]
    fn reads_workspace_targets_features_dependencies_and_profiles() {
        let root = std::env::temp_dir().join(format!("riga-cargo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [
            "crates/core/src",
            "crates/cli/src/bin/tool",
            "crates/cli/examples",
            "x/src",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\", \"x\", \"g/**\"]\n[profile.release]\nlto = true\n\
             [profile.fast.package.foo]\nopt-level = 3\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/core/Cargo.toml"),
            "[package]\nname = \"core\"\n[features]\ndefault = [\"std\"]\nstd = []\n\
             [dependencies]\nserde = \"1\"\nfoo.workspace = true\n[dependencies.bar]\nversion = \"1\"\n\
             [dev-dependencies]\ntempfile = \"3\"\n[target.'cfg(unix)'.build-dependencies]\ncc = \"1\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/cli/Cargo.toml"),
            "[package]\nname = \"cli\"\n[[bin]]\nname = \"extra\"\npath = \"e.rs\"\n[features]\nfancy = []\n",
        )
        .unwrap();
        fs::write(root.join("crates/cli/src/main.rs"), "").unwrap();
        fs::write(root.join("crates/cli/src/bin/one.rs"), "").unwrap();
        fs::write(root.join("crates/cli/src/bin/tool/main.rs"), "").unwrap();
        fs::write(root.join("crates/cli/examples/demo.rs"), "").unwrap();
        fs::write(root.join("crates/cli/examples/notes.txt"), "").unwrap();
        fs::write(root.join("x/Cargo.toml"), "[package]\nname = \"x-pkg\"\n").unwrap();
        fs::write(
            root.join("Cargo.lock"),
            "version = 4\n[[package]]\nname = \"cli\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"serde\"\n",
        )
        .unwrap();
        let cli = root.join("crates/cli/src");
        let core = root.join("crates/core");
        assert_eq!(names(&cli, Kind::Package), ["cli", "core", "x-pkg"]);
        assert_eq!(
            names(&root, Kind::LockPackage),
            ["cli", "core", "serde", "x-pkg"]
        );
        assert_eq!(names(&cli, Kind::Bin), ["cli", "extra", "one", "tool"]);
        assert_eq!(refs(&cli, Kind::Bin)[0].description, "Bin of cli");
        assert_eq!(names(&cli, Kind::Example), ["demo"]);
        assert_eq!(names(&cli, Kind::Feature), ["fancy"]);
        // At the workspace root every member contributes.
        assert_eq!(names(&root, Kind::Feature), ["default", "fancy", "std"]);
        assert_eq!(refs(&core, Kind::Feature)[0].description, "std");
        assert_eq!(
            names(&core, Kind::Dependency),
            ["bar", "cc", "foo", "serde", "tempfile"]
        );
        assert_eq!(
            refs(&core, Kind::Dependency)[4].description,
            "Dev-dependencies"
        );
        assert_eq!(
            names(&cli, Kind::Profile),
            ["bench", "dev", "fast", "release", "test"]
        );
        assert!(refs(Path::new("/"), Kind::Package).is_empty());
        fs::create_dir_all(root.join(".cargo")).unwrap();
        fs::write(
            root.join(".cargo/.crates.toml"),
            "[v1]\n\"ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)\" = [\"rg\"]\n",
        )
        .unwrap();
        let tools = refs(&root, Kind::Installed);
        assert_eq!(tools[0].name, "ripgrep");
        assert_eq!(tools[0].description, "14.1.0 rg");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn hostile_manifests_neither_crash_nor_hang() {
        let root = std::env::temp_dir().join(format!("riga-cargo-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // A basic string ending in a backslash at EOF used to index out of range.
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"app\"\ndescription = \"abc\\",
        )
        .unwrap();
        assert_eq!(names(&root, Kind::Package), ["app"]);
        // TOML 1.1 multi-line inline tables must not hide what follows.
        fs::write(
            root.join("Cargo.toml"),
            "[dependencies]\nserde = {\n version = \"1\",\n}\nrand = \"0.8\"\n[features]\nfast = []\n",
        )
        .unwrap();
        assert_eq!(names(&root, Kind::Dependency), ["rand", "serde"]);
        assert_eq!(names(&root, Kind::Feature), ["fast"]);
        // A FIFO named Cargo.toml is not a manifest and must not block the worker.
        fs::remove_file(root.join("Cargo.toml")).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(root.join("Cargo.toml"))
                .status()
                .unwrap()
                .success()
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = root.clone();
        std::thread::spawn(move || tx.send(names(&dir, Kind::Package)));
        let found = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("hung");
        assert!(found.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
