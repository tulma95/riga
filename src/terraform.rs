//! Terraform workspaces, variable files and resource addresses read from the working directory;
//! never runs terraform (it starts provider plugins).
use crate::{
    fsio::{self, read_text},
    json::Json,
    names::{Names, Ref},
    sources::Env,
};
use std::path::{Path, PathBuf};
const MAX_STATE_BYTES: u64 = 16 << 20;
const MAX_ENTRIES: usize = 1_000;
const MAX_NAMES: usize = 5_000;
const WORKSPACES: &str = "terraform.tfstate.d";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Workspace,
    VarFile,
    Resource,
}
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let mut out = Names::new(MAX_NAMES);
    match kind {
        Kind::Workspace => {
            let current = current_workspace(env.cwd);
            let mut push = |name: &str| {
                let description = if *name == current {
                    "Current workspace"
                } else {
                    "Workspace"
                };
                out.push(name, description);
            };
            push("default");
            for e in dirs(&env.cwd.join(WORKSPACES)) {
                push(&e);
            }
        }
        // ponytail: the working directory and its immediate subdirectories only.
        Kind::VarFile => {
            let mut add = |dir: &Path, prefix: &str| {
                for e in fsio::list_dir(dir, MAX_ENTRIES).into_iter().flatten() {
                    if e.name.ends_with(".tfvars") || e.name.ends_with(".tfvars.json") {
                        out.push(&format!("{prefix}{}", e.name), "Variable file");
                    }
                }
            };
            add(env.cwd, "");
            for sub in dirs(env.cwd) {
                add(&env.cwd.join(&sub), &format!("{sub}/"));
            }
        }
        Kind::Resource => {
            if let Some(text) = read_text(&state_path(env.cwd), MAX_STATE_BYTES) {
                resources(&text, &mut out);
            }
        }
    }
    out.finish()
}
/// Visible subdirectory names, skipping `.terraform` and other dot directories.
fn dirs(dir: &Path) -> Vec<String> {
    fsio::list_dir(dir, MAX_ENTRIES)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type.is_dir() && !e.name.starts_with('.'))
        .map(|e| e.name)
        .collect()
}
/// The workspace `terraform workspace select` last chose, from `.terraform/environment`.
fn current_workspace(cwd: &Path) -> String {
    read_text(&cwd.join(".terraform/environment"), 1024)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into())
}
/// The local backend's state file for the current workspace.
/// ponytail: local backend only; remote backends and `-state`/`backend "local" { path }` are not seen.
fn state_path(cwd: &Path) -> PathBuf {
    let ws = current_workspace(cwd);
    let safe = !ws.contains('/') && ws != "." && ws != "..";
    if ws == "default" || !safe {
        cwd.join("terraform.tfstate")
    } else {
        cwd.join(WORKSPACES).join(ws).join("terraform.tfstate")
    }
}
/// Resource and instance addresses from a v4 state file's `resources` list, such as
/// `module.net.aws_subnet.private["a"]` and `data.aws_ami.ubuntu`. A truncated or malformed file
/// keeps the addresses read before the error.
fn resources(text: &str, out: &mut Names) {
    let _ = Json::new(text).object(&mut |p, key| {
        if key != "resources" {
            return p.skip();
        }
        p.array(&mut |p| {
            if !p.at_object() {
                return p.skip();
            }
            let (mut module, mut mode, mut ty, mut name) = (None, None, None, None);
            let mut keys = vec![];
            p.object(&mut |p, key| {
                match key.as_str() {
                    "module" => module = p.string_or_skip()?,
                    "mode" => mode = p.string_or_skip()?,
                    "type" => ty = p.string_or_skip()?,
                    "name" => name = p.string_or_skip()?,
                    "instances" => p.array(&mut |p| {
                        if !p.at_object() {
                            return p.skip();
                        }
                        p.object(&mut |p, key| {
                            if key != "index_key" {
                                return p.skip();
                            }
                            // index_key is a string (for_each) or a number (count).
                            if p.at_string() {
                                let s = p.string()?;
                                if !s.contains(['"', '\\']) {
                                    keys.push(format!("[\"{s}\"]"));
                                }
                            } else {
                                let n = p.literal()?;
                                if n.bytes().all(|b| b.is_ascii_digit()) {
                                    keys.push(format!("[{n}]"));
                                }
                            }
                            Some(())
                        })
                    })?,
                    _ => p.skip()?,
                }
                Some(())
            })?;
            let (Some(ty), Some(name)) = (ty, name) else {
                return Some(());
            };
            let mut address = module.map(|m| m + ".").unwrap_or_default();
            if mode.as_deref() == Some("data") {
                address += "data.";
            }
            address += &format!("{ty}.{name}");
            out.push(&address, "Resource in local state");
            for key in keys {
                out.push(&format!("{address}{key}"), "Instance in local state");
            }
            Some(())
        })
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::with_env;
    use std::fs;
    fn names(cwd: &Path, kind: Kind) -> Vec<(String, String)> {
        with_env(cwd, cwd, "", |env| complete(env, kind))
            .into_iter()
            .map(|r| (r.name, r.description))
            .collect()
    }
    #[test]
    fn workspaces_var_files_and_state_addresses() {
        let dir = std::env::temp_dir().join(format!("riga-terraform-{}", std::process::id()));
        for d in [
            "terraform.tfstate.d/staging",
            "terraform.tfstate.d/prod",
            ".terraform",
            "envs",
        ] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in [
            "dev.tfvars",
            "x.auto.tfvars.json",
            "main.tf",
            "envs/prod.tfvars",
        ] {
            fs::write(dir.join(f), "").unwrap();
        }
        let state = r#"{"version": 4, "outputs": {"resources": []}, "resources": [
            {"mode": "managed", "type": "aws_instance", "name": "web", "provider": "p",
             "instances": [{"index_key": 0, "attributes": {"id": "i"}}, {"index_key": 1}]},
            {"module": "module.net", "mode": "data", "type": "aws_ami", "name": "ubuntu",
             "instances": [{"attributes": {}}]},
            {"mode": "managed", "type": "aws_subnet", "name": "s", "instances": [{"index_key": "a"}]},
            {"mode": "managed", "type": "cut", "name": "#;
        fs::write(dir.join("terraform.tfstate"), state).unwrap();
        let ws = names(&dir, Kind::Workspace);
        assert_eq!(
            ws,
            [
                ("default".into(), "Current workspace".into()),
                ("prod".into(), "Workspace".into()),
                ("staging".into(), "Workspace".into()),
            ]
        );
        let files: Vec<_> = names(&dir, Kind::VarFile)
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(
            files,
            ["dev.tfvars", "envs/prod.tfvars", "x.auto.tfvars.json"]
        );
        let addresses: Vec<_> = names(&dir, Kind::Resource)
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(
            addresses,
            [
                "aws_instance.web",
                "aws_instance.web[0]",
                "aws_instance.web[1]",
                "aws_subnet.s",
                "aws_subnet.s[\"a\"]",
                "module.net.data.aws_ami.ubuntu",
            ]
        );
        // The selected workspace has its own state file.
        fs::write(dir.join(".terraform/environment"), "staging\n").unwrap();
        assert_eq!(names(&dir, Kind::Workspace)[2].1, "Current workspace");
        assert!(names(&dir, Kind::Resource).is_empty());
        fs::write(
            dir.join("terraform.tfstate.d/staging/terraform.tfstate"),
            r#"{"resources": [{"type": "null_resource", "name": "n"}]}"#,
        )
        .unwrap();
        assert_eq!(names(&dir, Kind::Resource)[0].0, "null_resource.n");
        let missing = dir.join("missing");
        assert_eq!(names(&missing, Kind::Workspace).len(), 1);
        assert!(names(&missing, Kind::Resource).is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
