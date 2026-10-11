//! Git refs read directly from the repository files; never runs `git` or touches the network.
use crate::{
    fsio::{self, read_text},
    names::{Names, Ref},
    sources::Env,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
const MAX_REFS: usize = 20_000;
const MAX_PACKED_BYTES: u64 = 16 << 20;
/// Repository (`git dir`, `common dir`) containing `cwd`, following `.git` files of worktrees and submodules.
fn repository(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    let dir = cwd.ancestors().find_map(|dir| {
        let dot = dir.join(".git");
        if dot.is_dir() {
            return Some(dot);
        }
        let text = read_text(&dot, 4096)?;
        let target = text.strip_prefix("gitdir:")?.trim();
        Some(dir.join(target))
    })?;
    let common = read_text(&dir.join("commondir"), 4096)
        .map(|c| dir.join(c.trim()))
        .unwrap_or_else(|| dir.clone());
    Some((dir, common))
}
/// Full ref names (`refs/heads/main`) from loose files and `packed-refs`.
fn refs(common: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![common.join("refs")];
    // ponytail: rescans per request; cache by packed-refs/dir mtimes if huge repos feel slow.
    while let Some(dir) = stack.pop() {
        let Some(entries) = fsio::list_dir(&dir, fsio::MAX_DIR_ENTRIES) else {
            continue;
        };
        for e in entries {
            if out.len() >= MAX_REFS {
                return out;
            }
            let path = dir.join(&e.name);
            if e.file_type.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(common)
                && let Some(name) = rel.to_str()
            {
                out.insert(name.to_owned());
            }
        }
    }
    for line in read_text(&common.join("packed-refs"), MAX_PACKED_BYTES)
        .unwrap_or_default()
        .lines()
    {
        if let Some((_, name)) = line.split_once(' ')
            && !line.starts_with(['#', '^'])
            && out.len() < MAX_REFS
        {
            out.insert(name.to_owned());
        }
    }
    out
}
fn current(dir: &Path) -> Option<String> {
    let head = read_text(&dir.join("HEAD"), 4096)?;
    Some(head.strip_prefix("ref: refs/heads/")?.trim().to_owned())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Local branches, plus remote-only branches by the short name `git switch` resolves.
    Branch,
    /// Branches, remote-tracking branches and tags, as revision names.
    Ref,
    Remote,
}
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let Some((dir, common)) = repository(env.cwd) else {
        return vec![];
    };
    let all = refs(&common);
    let head = current(&dir);
    let mut out = Names::new(MAX_REFS);
    let mut local = BTreeSet::new();
    // A remote-only short name can exist on several remotes; the first one wins.
    let mut add = |name: &str, description: String| out.push(name, &description);
    for name in all.iter().filter_map(|r| r.strip_prefix("refs/heads/")) {
        local.insert(name);
        if matches!(kind, Kind::Branch | Kind::Ref) {
            let description = if head.as_deref() == Some(name) {
                "Current branch"
            } else {
                "Local branch"
            };
            add(name, description.into());
        }
    }
    let mut remotes = BTreeSet::new();
    for full in all.iter().filter_map(|r| r.strip_prefix("refs/remotes/")) {
        let Some((remote, branch)) = full.split_once('/') else {
            continue;
        };
        remotes.insert(remote);
        match kind {
            _ if branch == "HEAD" => {}
            Kind::Branch if !local.contains(branch) => add(branch, format!("Remote branch {full}")),
            Kind::Ref => add(full, "Remote-tracking branch".into()),
            _ => {}
        }
    }
    match kind {
        Kind::Ref => {
            for tag in all.iter().filter_map(|r| r.strip_prefix("refs/tags/")) {
                add(tag, "Tag".into());
            }
        }
        Kind::Remote => {
            // Remotes without fetched branches still appear in the configuration.
            let config = read_text(&common.join("config"), 1 << 20).unwrap_or_default();
            for line in config.lines() {
                if let Some(name) = line
                    .trim()
                    .strip_prefix("[remote \"")
                    .and_then(|r| r.strip_suffix("\"]"))
                {
                    remotes.insert(name);
                }
            }
            for remote in remotes {
                add(remote, "Remote".into());
            }
        }
        Kind::Branch => {}
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
    fn reads_loose_packed_remote_and_worktree_refs() {
        let root = std::env::temp_dir().join(format!("riga-git-{}", std::process::id()));
        let git = root.join("repo/.git");
        for dir in [
            "refs/heads/feature",
            "refs/remotes/origin",
            "refs/tags",
            "wt",
        ] {
            fs::create_dir_all(git.join(dir)).unwrap();
        }
        fs::create_dir_all(root.join("repo/sub/deeper")).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        for r in [
            "heads/main",
            "heads/feature/login",
            "remotes/origin/HEAD",
            "tags/v1",
        ] {
            fs::write(git.join("refs").join(r), "0\n").unwrap();
        }
        fs::write(
            git.join("packed-refs"),
            "# pack-refs with: peeled\n1 refs/remotes/origin/main\n2 refs/remotes/origin/only-remote\n^3\n",
        )
        .unwrap();
        fs::write(
            git.join("config"),
            "[remote \"origin\"]\n[remote \"backup\"]\n",
        )
        .unwrap();
        let sub = root.join("repo/sub/deeper");
        assert_eq!(
            names(&sub, Kind::Branch),
            ["feature/login", "main", "only-remote"]
        );
        let all = complete_at(&sub, Kind::Branch);
        assert_eq!(all[1].description, "Current branch");
        assert_eq!(all[2].description, "Remote branch origin/only-remote");
        assert_eq!(
            names(&sub, Kind::Ref),
            [
                "feature/login",
                "main",
                "origin/main",
                "origin/only-remote",
                "v1"
            ]
        );
        assert_eq!(names(&sub, Kind::Remote), ["backup", "origin"]);
        // A linked worktree: `.git` file -> private git dir whose commondir is the main repository.
        fs::write(git.join("wt/HEAD"), "ref: refs/heads/feature/login\n").unwrap();
        fs::write(git.join("wt/commondir"), "..\n").unwrap();
        fs::create_dir_all(root.join("wt")).unwrap();
        fs::write(
            root.join("wt/.git"),
            format!("gitdir: {}\n", git.join("wt").display()),
        )
        .unwrap();
        let wt = complete_at(&root.join("wt"), Kind::Branch);
        assert_eq!(wt[0].description, "Current branch");
        assert_eq!(wt.len(), 3);
        assert!(complete_at(Path::new("/"), Kind::Branch).is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
