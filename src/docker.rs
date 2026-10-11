//! Docker container and image names from the current context, listed in the background.
use crate::{
    names::{Names, Ref},
    sources::Env,
};
use std::time::Duration;
const MAX_NAMES: usize = 5_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Container,
    Running,
    Image,
}
// ponytail: the daemon's own environment and current context; the shell's $DOCKER_HOST and -H/--context
// in the buffer are ignored.
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let argv: &[&str] = match kind {
        Kind::Container => &["docker", "ps", "-a", "--format", "{{.Names}}\t{{.Status}}"],
        Kind::Running => &["docker", "ps", "--format", "{{.Names}}\t{{.Status}}"],
        Kind::Image => &[
            "docker",
            "images",
            "--format",
            "{{.Repository}}:{{.Tag}}\t{{.Size}}",
        ],
    };
    parse(&env.runs.lines(env.path, argv, Duration::from_secs(5)))
}
/// `name<TAB>description` lines; dangling images (`<none>:<none>`) have no usable name.
fn parse(lines: &[String]) -> Vec<Ref> {
    let mut out = Names::new(MAX_NAMES);
    for l in lines {
        let (name, description) = l.split_once('\t').unwrap_or((l, ""));
        if !name.contains("<none>") {
            out.push(name, description);
        }
    }
    out.finish()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_names_and_skips_dangling_images() {
        let lines = [
            "web-1\tUp 2 hours".to_owned(),
            "ubuntu:24.04\t78MB".into(),
            "<none>:<none>\t1GB".into(),
            "app:<none>\t5MB".into(),
        ];
        let pairs: Vec<_> = parse(&lines)
            .into_iter()
            .map(|r| (r.name, r.description))
            .collect();
        assert_eq!(
            pairs,
            [
                ("ubuntu:24.04".into(), "78MB".into()),
                ("web-1".into(), "Up 2 hours".into())
            ]
        );
    }
}
