//! The built-in value sources a `.riga` spec can name (`<git-branch>`, `<cargo-package>`, ...): one
//! token table, and exhaustive dispatch and path rules. Sources read local files (or, for tmux, ffmpeg
//! and docker, cached background runs) and never depend on the spec model.
use crate::{
    aws, background::Runs, brew, cargo_toml, cdk, docker, ffmpeg, git, kube, names::Ref, npm,
    ollama, pyproject, rustup, terraform, tmux, vscode,
};
use std::path::Path;
/// Everything a source may consult; all of it comes from the request or the engine, never from the
/// daemon's own environment, so sources are injectable and testable.
pub struct Env<'a> {
    pub cwd: &'a Path,
    pub home: &'a Path,
    /// The shell's PATH, for sources that locate a tool (`brew`, `tmux`, `ffmpeg`, `docker`).
    pub path: &'a str,
    pub runs: &'a Runs,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Git(git::Kind),
    Npm(npm::Kind),
    Cargo(cargo_toml::Kind),
    Py(pyproject::Kind),
    Brew(brew::Kind),
    Kube(kube::Kind),
    Ollama,
    Rustup(rustup::Kind),
    Vscode(vscode::Kind),
    Tmux(tmux::Kind),
    Ffmpeg(ffmpeg::Kind),
    Docker(docker::Kind),
    Terraform(terraform::Kind),
    Aws(aws::Kind),
    CdkStack,
}
/// Spec value tokens (`<token>`) and their sources.
const TOKENS: &[(&str, Source)] = &[
    ("git-branch", Source::Git(git::Kind::Branch)),
    ("git-ref", Source::Git(git::Kind::Ref)),
    ("git-remote", Source::Git(git::Kind::Remote)),
    ("npm-script", Source::Npm(npm::Kind::Script)),
    ("npm-dependency", Source::Npm(npm::Kind::Dependency)),
    ("npm-bin", Source::Npm(npm::Kind::Bin)),
    ("npm-workspace", Source::Npm(npm::Kind::Workspace)),
    ("cargo-package", Source::Cargo(cargo_toml::Kind::Package)),
    (
        "cargo-lock-package",
        Source::Cargo(cargo_toml::Kind::LockPackage),
    ),
    ("cargo-bin", Source::Cargo(cargo_toml::Kind::Bin)),
    ("cargo-example", Source::Cargo(cargo_toml::Kind::Example)),
    ("cargo-test", Source::Cargo(cargo_toml::Kind::Test)),
    ("cargo-bench", Source::Cargo(cargo_toml::Kind::Bench)),
    ("cargo-feature", Source::Cargo(cargo_toml::Kind::Feature)),
    (
        "cargo-dependency",
        Source::Cargo(cargo_toml::Kind::Dependency),
    ),
    ("cargo-profile", Source::Cargo(cargo_toml::Kind::Profile)),
    (
        "cargo-installed",
        Source::Cargo(cargo_toml::Kind::Installed),
    ),
    ("py-dependency", Source::Py(pyproject::Kind::Dependency)),
    ("py-group", Source::Py(pyproject::Kind::Group)),
    ("py-extra", Source::Py(pyproject::Kind::Extra)),
    ("py-script", Source::Py(pyproject::Kind::Script)),
    ("uv-tool", Source::Py(pyproject::Kind::Tool)),
    ("brew-package", Source::Brew(brew::Kind::Package)),
    ("brew-formula", Source::Brew(brew::Kind::Formula)),
    ("brew-installed", Source::Brew(brew::Kind::Installed)),
    (
        "brew-installed-formula",
        Source::Brew(brew::Kind::InstalledFormula),
    ),
    ("brew-pinned", Source::Brew(brew::Kind::Pinned)),
    ("brew-tap", Source::Brew(brew::Kind::Tap)),
    ("kube-context", Source::Kube(kube::Kind::Context)),
    ("kube-cluster", Source::Kube(kube::Kind::Cluster)),
    ("kube-user", Source::Kube(kube::Kind::User)),
    ("kube-namespace", Source::Kube(kube::Kind::Namespace)),
    ("ollama-model", Source::Ollama),
    ("rustup-toolchain", Source::Rustup(rustup::Kind::Toolchain)),
    ("rustup-target", Source::Rustup(rustup::Kind::Target)),
    ("rustup-component", Source::Rustup(rustup::Kind::Component)),
    ("vscode-extension", Source::Vscode(vscode::Kind::Extension)),
    ("vscode-profile", Source::Vscode(vscode::Kind::Profile)),
    ("tmux-session", Source::Tmux(tmux::Kind::Session)),
    ("tmux-window", Source::Tmux(tmux::Kind::Window)),
    ("tmux-pane", Source::Tmux(tmux::Kind::Pane)),
    ("tmux-buffer", Source::Tmux(tmux::Kind::Buffer)),
    ("ffmpeg-encoder", Source::Ffmpeg(ffmpeg::Kind::Encoder)),
    (
        "ffmpeg-video-encoder",
        Source::Ffmpeg(ffmpeg::Kind::VideoEncoder),
    ),
    (
        "ffmpeg-audio-encoder",
        Source::Ffmpeg(ffmpeg::Kind::AudioEncoder),
    ),
    (
        "ffmpeg-subtitle-encoder",
        Source::Ffmpeg(ffmpeg::Kind::SubtitleEncoder),
    ),
    ("docker-container", Source::Docker(docker::Kind::Container)),
    (
        "docker-running-container",
        Source::Docker(docker::Kind::Running),
    ),
    ("docker-image", Source::Docker(docker::Kind::Image)),
    ("aws-profile", Source::Aws(aws::Kind::Profile)),
    ("aws-sso-session", Source::Aws(aws::Kind::SsoSession)),
    ("cdk-stack", Source::CdkStack),
    ("ffmpeg-format", Source::Ffmpeg(ffmpeg::Kind::Format)),
    (
        "ffmpeg-video-filter",
        Source::Ffmpeg(ffmpeg::Kind::VideoFilter),
    ),
    (
        "ffmpeg-audio-filter",
        Source::Ffmpeg(ffmpeg::Kind::AudioFilter),
    ),
    (
        "terraform-workspace",
        Source::Terraform(terraform::Kind::Workspace),
    ),
    (
        "terraform-var-file",
        Source::Terraform(terraform::Kind::VarFile),
    ),
    (
        "terraform-resource",
        Source::Terraform(terraform::Kind::Resource),
    ),
];
impl Source {
    pub fn parse(token: &str) -> Option<Source> {
        TOKENS.iter().find(|(t, _)| *t == token).map(|&(_, s)| s)
    }
    /// Whether `label` starts like a source token (`git-...`), which marks a typo when it is not one.
    pub fn family(label: &str) -> bool {
        let prefix = label.split_once('-').map(|(prefix, _)| prefix);
        prefix.is_some_and(|p| TOKENS.iter().any(|(t, _)| t.split('-').next() == Some(p)))
    }
    /// Candidates sorted by name and capped; empty when the underlying data is missing.
    pub fn complete(self, env: &Env) -> Vec<Ref> {
        match self {
            Source::Git(k) => git::complete(env, k),
            Source::Npm(k) => npm::complete(env, k),
            Source::Cargo(k) => cargo_toml::complete(env, k),
            Source::Py(k) => pyproject::complete(env, k),
            Source::Brew(k) => brew::complete(env, k),
            Source::Kube(k) => kube::complete(env, k),
            Source::Ollama => ollama::complete(env),
            Source::Rustup(k) => rustup::complete(env, k),
            Source::Vscode(k) => vscode::complete(env, k),
            Source::Tmux(k) => tmux::complete(env, k),
            Source::Ffmpeg(k) => ffmpeg::complete(env, k),
            Source::Docker(k) => docker::complete(env, k),
            Source::Terraform(k) => terraform::complete(env, k),
            Source::Aws(k) => aws::complete(env, k),
            Source::CdkStack => cdk::stacks(env),
        }
    }
    /// The suggestion kind shown for its candidates.
    pub fn suggestion_kind(self) -> &'static str {
        match self {
            Source::Git(_) => "ref",
            _ => "value",
        }
    }
    /// Whether file system paths are offered next to the candidates. Positionals that may also name a
    /// file (`git checkout -- f`, `npx ./script.js`, `uv run script.py`) say yes; flag values and names
    /// that are never paths say no.
    pub fn offers_paths(self, is_flag: bool) -> bool {
        match self {
            Source::Git(_) | Source::Npm(npm::Kind::Bin) | Source::Py(pyproject::Kind::Script) => {
                !is_flag
            }
            Source::Npm(npm::Kind::Script | npm::Kind::Dependency | npm::Kind::Workspace)
            | Source::Cargo(_)
            | Source::Py(
                pyproject::Kind::Dependency
                | pyproject::Kind::Group
                | pyproject::Kind::Extra
                | pyproject::Kind::Tool,
            )
            | Source::Brew(_)
            | Source::Kube(_)
            | Source::Ollama
            | Source::Rustup(_)
            | Source::Vscode(_)
            | Source::Tmux(_)
            | Source::Ffmpeg(_)
            | Source::Docker(_)
            | Source::Terraform(_)
            | Source::Aws(_)
            | Source::CdkStack => false,
        }
    }
}
/// Runs `f` with an environment for tests.
#[cfg(test)]
pub fn with_env<R>(cwd: &Path, home: &Path, path: &str, f: impl FnOnce(&Env) -> R) -> R {
    f(&Env {
        cwd,
        home,
        path,
        runs: &Runs::default(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokens_are_unique_and_parse_back() {
        for (i, (token, source)) in TOKENS.iter().enumerate() {
            assert_eq!(Source::parse(token), Some(*source));
            assert!(
                TOKENS[..i].iter().all(|(t, s)| t != token && s != source),
                "{token}"
            );
        }
        assert_eq!(Source::parse("branch"), None);
    }
    #[test]
    fn missing_data_gives_only_built_in_candidates() {
        let nowhere = Path::new("/nonexistent-riga");
        for (token, source) in TOKENS {
            // Brew falls back to the real prefixes and the others start background runs.
            if matches!(
                source,
                Source::Brew(_) | Source::Tmux(_) | Source::Ffmpeg(_)
            ) {
                continue;
            }
            let found = with_env(nowhere, nowhere, "", |env| source.complete(env));
            // Release channels and built-in profiles.
            assert!(found.len() <= 4, "{token}");
        }
    }
}
