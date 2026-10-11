//! uv values: dependency, extra, group and script names from the nearest `pyproject.toml`, and installed
//! tool names from the uv tools directory. Never runs uv, Python or project code.
use crate::{
    fsio::{self, read_text},
    names::{Names, Ref},
    sources::Env,
    toml::{self, Value},
};
use std::path::Path;
const MAX_BYTES: u64 = 1 << 20;
const MAX_NAMES: usize = 5_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dependency,
    Group,
    Extra,
    Script,
    /// Tools installed with `uv tool install`.
    Tool,
}
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let mut names = Names::new(MAX_NAMES);
    match kind {
        Kind::Tool => tools(env.home, &mut names),
        Kind::Dependency => dependencies(&project(env.cwd), &mut names),
        Kind::Group => {
            let doc = project(env.cwd);
            // uv's legacy `dev-dependencies` are the `dev` dependency group.
            let legacy = doc.at(&["tool", "uv", "dev-dependencies"]).map(|_| "dev");
            let groups = doc
                .get("dependency-groups")
                .into_iter()
                .flat_map(Value::keys);
            for group in legacy.into_iter().chain(groups) {
                names.push(group, "Dependency group");
            }
        }
        Kind::Extra => {
            let doc = project(env.cwd);
            for extra in doc
                .at(&["project", "optional-dependencies"])
                .into_iter()
                .flat_map(Value::keys)
            {
                names.push(extra, "Optional dependency extra");
            }
        }
        Kind::Script => {
            let doc = project(env.cwd);
            for (script, target) in doc
                .at(&["project", "scripts"])
                .into_iter()
                .flat_map(Value::entries)
            {
                names.push(script, target.as_str().unwrap_or_default());
            }
        }
    }
    names.finish()
}
/// The nearest ancestor with a pyproject.toml is the (workspace member) project uv uses; an empty
/// document when there is none.
fn project(cwd: &Path) -> Value {
    cwd.ancestors()
        .find_map(|dir| read_text(&dir.join("pyproject.toml"), MAX_BYTES))
        .map_or(Value::Other, |text| toml::parse(&text))
}
fn dependencies(doc: &Value, names: &mut Names) {
    let extras = doc
        .at(&["project", "optional-dependencies"])
        .into_iter()
        .flat_map(Value::entries)
        .map(|(extra, list)| (format!("Extra {extra}"), list));
    let groups = doc
        .get("dependency-groups")
        .into_iter()
        .flat_map(Value::entries)
        .map(|(group, list)| (format!("Group {group}"), list));
    let legacy = doc
        .at(&["tool", "uv", "dev-dependencies"])
        .map(|list| ("Group dev".to_owned(), list));
    let project = doc
        .at(&["project", "dependencies"])
        .map(|list| ("Dependencies".to_owned(), list));
    for (description, list) in project
        .into_iter()
        .chain(extras)
        .chain(legacy)
        .chain(groups)
    {
        for item in list.strings() {
            // PEP 508: the name is the leading run before extras, versions or markers.
            let item = item.trim_start();
            let end = item
                .find(|c: char| !(c.is_ascii_alphanumeric() || "._-".contains(c)))
                .unwrap_or(item.len());
            names.push(&item[..end], &description);
        }
    }
}
/// Installed tools are the directories of `~/.local/share/uv/tools`.
/// ponytail: ignores $UV_TOOL_DIR and $XDG_DATA_HOME because `State` carries no shell environment.
fn tools(home: &Path, names: &mut Names) {
    let dir = home.join(".local/share/uv/tools");
    for e in fsio::list_dir(&dir, fsio::MAX_DIR_ENTRIES)
        .into_iter()
        .flatten()
    {
        if e.file_type.is_dir() && !e.name.starts_with('.') {
            names.push(&e.name, "Installed tool");
        }
    }
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
    fn reads_names_from_nearest_pyproject() {
        let root = std::env::temp_dir().join(format!("riga-pyproject-{}", std::process::id()));
        fs::create_dir_all(root.join("app/src/deep")).unwrap();
        fs::write(
            root.join("app/pyproject.toml"),
            r#"# comment
[project]
name = "app"  # trailing
description = """Multi "quoted"
line = [not, a, key]"""
version = '0.1'
dependencies = [
  "requests[socks]>=2.31 ; python_version >= '3.9'", # comment
  'rich',
  "BIDIevil",
  "",
]
optional-dependencies.cli = ["click>=8"]
authors = [{ name = "A", email = "a@example.com" }]

[project.optional-dependencies]
docs = ["sphinx", "requests"]
"test.all" = []

[project.scripts]
app = "app.cli:main"
"app-admin" = "app.admin:main"
"xBIDIy" = "a"
hidden = "BIDIb"

[[tool.ruff.overrides]]
scripts = { x = "y" }

[tool.uv]
dev-dependencies = ["pytest>=8"]

[dependency-groups]
lint = ["ruff==0.6.*", { include-group = "typing" }]
typing = { }
"#
            .replace("BIDI", "\u{202e}"),
        )
        .unwrap();
        let deep = root.join("app/src/deep");
        assert_eq!(
            names(&deep, Kind::Dependency),
            ["click", "pytest", "requests", "rich", "ruff", "sphinx"]
        );
        let deps = complete_at(&deep, Kind::Dependency);
        assert_eq!(deps[0].description, "Extra cli");
        assert_eq!(deps[2].description, "Dependencies");
        assert_eq!(names(&deep, Kind::Extra), ["cli", "docs", "test.all"]);
        assert_eq!(names(&deep, Kind::Group), ["dev", "lint", "typing"]);
        assert_eq!(names(&deep, Kind::Script), ["app", "app-admin", "hidden"]);
        assert_eq!(complete_at(&deep, Kind::Script)[2].description, "");
        assert_eq!(
            complete_at(&deep, Kind::Script)[0].description,
            "app.cli:main"
        );
        // Malformed input keeps what was read before the error.
        fs::write(
            root.join("app/pyproject.toml"),
            "[project.scripts]\na = \"x\"\nb = \"unterminated\n",
        )
        .unwrap();
        assert_eq!(names(&deep, Kind::Script), ["a"]);
        // Multi-line inline tables (TOML 1.1) do not hide later tables.
        fs::write(
            root.join("app/pyproject.toml"),
            "[project]\nurls = {\n  home = \"x\",\n}\ndependencies = [\"six\"]\n[project.scripts]\nrun = \"a:b\"\n",
        )
        .unwrap();
        assert_eq!(names(&deep, Kind::Dependency), ["six"]);
        assert_eq!(names(&deep, Kind::Script), ["run"]);
        assert!(complete_at(Path::new("/"), Kind::Script).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn lists_installed_tools() {
        let home = std::env::temp_dir().join(format!("riga-uv-tools-{}", std::process::id()));
        let tools_dir = home.join(".local/share/uv/tools");
        for d in ["ruff", "black", ".cache"] {
            fs::create_dir_all(tools_dir.join(d)).unwrap();
        }
        fs::write(tools_dir.join(".lock"), "").unwrap();
        assert_eq!(names(&home, Kind::Tool), ["black", "ruff"]);
        assert!(names(&home.join("missing"), Kind::Tool).is_empty());
        fs::remove_dir_all(home).unwrap();
    }
}
