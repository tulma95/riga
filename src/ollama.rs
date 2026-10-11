//! Locally installed Ollama model names, read from `~/.ollama/models/manifests`; never runs ollama.
use crate::{
    fsio,
    names::{Names, Ref},
    sources::Env,
};
use std::path::{Path, PathBuf};
const MAX_ENTRIES: usize = 1_000;
const MAX_NAMES: usize = 2_000;
/// Models in `manifests/<registry>/<namespace>/<model>/<tag>`, named like `ollama list` names them.
/// ponytail: ignores `$OLLAMA_MODELS` (the shell environment is not sent to the daemon).
pub fn complete(env: &Env) -> Vec<Ref> {
    let mut out = Names::new(MAX_NAMES);
    for host in entries(&env.home.join(".ollama/models/manifests"), true) {
        for ns in entries(&host, true) {
            for model in entries(&ns, true) {
                for tag in entries(&model, false) {
                    let part = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
                    let (host, ns) = (part(&host), part(&ns));
                    let name = match (host.as_str(), ns.as_str()) {
                        ("registry.ollama.ai", "library") => String::new(),
                        ("registry.ollama.ai", _) => format!("{ns}/"),
                        _ => format!("{host}/{ns}/"),
                    } + &format!("{}:{}", part(&model), part(&tag));
                    out.push(&name, "Local model");
                }
            }
        }
    }
    out.finish()
}
/// Bounded listing of subdirectories (or files) whose names are safe to show and insert.
fn entries(dir: &Path, dirs: bool) -> Vec<PathBuf> {
    fsio::list_dir(dir, MAX_ENTRIES)
        .into_iter()
        .flatten()
        .filter(|e| {
            if dirs {
                e.file_type.is_dir()
            } else {
                e.file_type.is_file()
            }
        })
        .filter(|e| {
            !e.name.starts_with('.')
                && e.name.chars().all(|c| {
                    c.is_alphanumeric() && !crate::spec::deceptive(c) || "-_.:+@%".contains(c)
                })
        })
        .map(|e| dir.join(e.name))
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::with_env;
    use std::fs;
    fn complete_at(home: &Path) -> Vec<Ref> {
        with_env(home, home, "", complete)
    }
    #[test]
    fn manifest_tree_gives_display_names() {
        let home = std::env::temp_dir().join(format!("riga-ollama-{}", std::process::id()));
        let m = home.join(".ollama/models/manifests");
        for p in [
            "registry.ollama.ai/library/llama3/latest",
            "registry.ollama.ai/library/qwen3/8b",
            "registry.ollama.ai/someone/tool/v1",
            "example.com/team/m/latest",
            "registry.ollama.ai/library/bad name/latest",
        ] {
            fs::create_dir_all(m.join(p).parent().unwrap()).unwrap();
            fs::write(m.join(p), "{}").unwrap();
        }
        fs::create_dir_all(m.join("registry.ollama.ai/library/dir/notafile")).unwrap();
        let names: Vec<_> = complete_at(&home).into_iter().map(|r| r.name).collect();
        assert_eq!(
            names,
            [
                "example.com/team/m:latest",
                "llama3:latest",
                "qwen3:8b",
                "someone/tool:v1"
            ]
        );
        assert!(complete_at(&home.join("missing")).is_empty());
        fs::remove_dir_all(home).unwrap();
    }
}
