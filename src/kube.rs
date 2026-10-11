//! kubectl contexts, clusters, users and namespaces read from `~/.kube/config`; never runs kubectl
//! or contacts a cluster.
use crate::{
    fsio::read_text,
    names::{Names, Ref},
    sources::Env,
};
const MAX_BYTES: u64 = 1 << 20;
const MAX_NAMES: usize = 5_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Cluster,
    User,
    Namespace,
}
/// ponytail: the default kubeconfig only; `$KUBECONFIG` and `--kubeconfig` are not seen.
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    read_text(&env.home.join(".kube/config"), MAX_BYTES)
        .map(|text| parse(&text, kind))
        .unwrap_or_default()
}
/// Line-based reader for the block-style YAML that kubectl writes: a top-level `contexts:`,
/// `clusters:` or `users:` list whose items carry `name:` (and, for contexts, a `context:` mapping
/// with `namespace:`). Items are recognised by the column of their `-`, and an item's own keys by the
/// column after it, so nested lists (minikube `extensions`, EKS `exec.env`) never leak their `name:`.
/// ponytail: flow style, anchors, multi-line scalars and multi-document files are not understood.
fn parse(text: &str, kind: Kind) -> Vec<Ref> {
    let wanted = match kind {
        Kind::Context | Kind::Namespace => "contexts",
        Kind::Cluster => "clusters",
        Kind::User => "users",
    };
    let mut current = None;
    let mut section = "";
    // Column of the `-` that starts items of the section, and of the keys of the current item.
    let (mut dash, mut keys) = (None, 0);
    // Namespaces: inside the item's `context:` mapping, and the column of that mapping's keys.
    let (mut in_context, mut context_keys) = (false, None);
    let mut found = vec![];
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut col = line.len() - trimmed.len();
        let mut rest = trimmed;
        if col == 0 && !rest.starts_with('-') {
            let (key, value) = split(rest);
            section = key;
            dash = None;
            if key == "current-context" {
                current = value;
            }
            continue;
        }
        if section != wanted {
            continue;
        }
        let item_start = rest == "-" || rest.starts_with("- ");
        if item_start {
            let content = rest[1..].trim_start();
            let content_col = col + (rest.len() - content.len());
            if *dash.get_or_insert(col) == col {
                (keys, in_context, context_keys) = (content_col, false, None);
            }
            (col, rest) = (content_col, content);
        }
        let (key, value) = split(rest);
        if col == keys {
            match (key, kind) {
                ("name", Kind::Context | Kind::Cluster | Kind::User) => found.extend(value),
                ("context", Kind::Namespace) => (in_context, context_keys) = (true, None),
                _ => in_context = false,
            }
        } else if in_context && col > keys && !item_start {
            let at = *context_keys.get_or_insert(col);
            if col == at && key == "namespace" {
                found.extend(value);
            }
        }
    }
    let label = match kind {
        Kind::Context => "Kubeconfig context",
        Kind::Cluster => "Kubeconfig cluster",
        Kind::User => "Kubeconfig user",
        Kind::Namespace => "Namespace used by a kubeconfig context",
    };
    let mut names = Names::new(MAX_NAMES);
    for name in &found {
        let is_current = kind == Kind::Context && current.as_deref() == Some(name);
        names.push(
            name,
            if is_current {
                "Current kubeconfig context"
            } else {
                label
            },
        );
    }
    names.finish()
}
/// `key: value`: the value is a plain, `'single'` (`''` is a quote) or `"double"` (backslash escapes)
/// scalar, with a trailing ` #` comment dropped from plain ones; `None` when empty or unterminated.
fn split(line: &str) -> (&str, Option<String>) {
    let Some((key, value)) = line.split_once(':') else {
        return (line.trim(), None);
    };
    (key.trim(), scalar(value.trim()))
}
fn scalar(value: &str) -> Option<String> {
    let mut chars = value.chars();
    let quote = match chars.next()? {
        q @ ('"' | '\'') => q,
        '#' => return None,
        _ => {
            let plain = value.split(" #").next().unwrap_or("").trim();
            return (!plain.is_empty()).then(|| plain.to_owned());
        }
    };
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quote == '"' => out.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                c => c,
            }),
            c if c == quote => {
                if quote == '\'' && chars.as_str().starts_with('\'') {
                    chars.next();
                    out.push('\'');
                } else {
                    return (!out.is_empty()).then_some(out);
                }
            }
            c => out.push(c),
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::with_env;
    use std::fs;
    const CONFIG: &str = "apiVersion: v1
clusters:
- cluster:
    server: https://example.invalid
    name: not-a-cluster
  name: prod-cluster
- name: \"dev-cluster\"
  cluster: {}
contexts:
  - context:
      cluster: prod-cluster
      namespace: payments # comment
      user: admin
    name: prod
  - context:
      cluster: dev-cluster
      user: 'dev'
    name: 'dev'
current-context: prod
kind: Config
users:
- name: admin
  user:
    token: secret
    name: nested
- name: \"bad\u{202e}name\"
";
    /// Written by `minikube start`: the context carries an `extensions` list with its own `name`.
    const MINIKUBE: &str = "apiVersion: v1
clusters:
- cluster:
    certificate-authority: /home/u/.minikube/ca.crt
    extensions:
    - extension:
        last-update: Mon, 01 Jan 2024 00:00:00 UTC
        provider: minikube.sigs.k8s.io
        version: v1.33.0
      name: cluster_info
    server: https://192.168.49.2:8443
  name: minikube
contexts:
- context:
    cluster: minikube
    extensions:
    - extension:
        last-update: Mon, 01 Jan 2024 00:00:00 UTC
        provider: minikube.sigs.k8s.io
        version: v1.33.0
      name: context_info
    namespace: default
    user: minikube
  name: minikube
current-context: minikube
kind: Config
preferences: {}
users:
- name: minikube
  user:
    client-certificate: /home/u/.minikube/profiles/minikube/client.crt
";
    /// Written by `aws eks update-kubeconfig`: the user's exec block has an `env` list of `name:` items.
    const EKS: &str = "apiVersion: v1
clusters:
- cluster:
    certificate-authority-data: DATA
    server: https://ABC.gr7.us-east-1.eks.amazonaws.com
  name: arn:aws:eks:us-east-1:123456789012:cluster/prod
contexts:
- context:
    cluster: arn:aws:eks:us-east-1:123456789012:cluster/prod
    user: arn:aws:eks:us-east-1:123456789012:cluster/prod
  name: arn:aws:eks:us-east-1:123456789012:cluster/prod
current-context: arn:aws:eks:us-east-1:123456789012:cluster/prod
kind: Config
users:
- name: arn:aws:eks:us-east-1:123456789012:cluster/prod
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      args:
      - --region
      - us-east-1
      command: aws
      env:
      - name: AWS_PROFILE
        value: work
      interactiveMode: IfAvailable
";
    fn names(text: &str, kind: Kind) -> Vec<String> {
        parse(text, kind).into_iter().map(|r| r.name).collect()
    }
    #[test]
    fn kubeconfig_names_by_kind() {
        assert_eq!(names(CONFIG, Kind::Context), ["dev", "prod"]);
        assert_eq!(
            names(CONFIG, Kind::Cluster),
            ["dev-cluster", "prod-cluster"]
        );
        assert_eq!(names(CONFIG, Kind::User), ["admin"]);
        assert_eq!(names(CONFIG, Kind::Namespace), ["payments"]);
        let contexts = parse(CONFIG, Kind::Context);
        assert_eq!(contexts[0].description, "Kubeconfig context");
        assert_eq!(contexts[1].description, "Current kubeconfig context");
    }
    #[test]
    fn nested_lists_do_not_leak_names() {
        assert_eq!(names(MINIKUBE, Kind::Context), ["minikube"]);
        assert_eq!(names(MINIKUBE, Kind::Cluster), ["minikube"]);
        assert_eq!(names(MINIKUBE, Kind::User), ["minikube"]);
        assert_eq!(names(MINIKUBE, Kind::Namespace), ["default"]);
        let eks = "arn:aws:eks:us-east-1:123456789012:cluster/prod";
        assert_eq!(names(EKS, Kind::User), [eks]);
        assert_eq!(names(EKS, Kind::Context), [eks]);
        assert!(names(EKS, Kind::Namespace).is_empty());
    }
    #[test]
    fn quoted_scalars() {
        let config = "contexts:
- name: 'it''s'
- name: \"a\\\"b\"
- name: \"foo #bar\" # comment
- name: plain # comment
- name: \"unterminated
- name:
";
        assert_eq!(
            names(config, Kind::Context),
            ["a\"b", "foo #bar", "it's", "plain"]
        );
    }
    #[test]
    fn kubeconfig_comes_from_the_injected_home() {
        let home = std::env::temp_dir().join(format!("riga-kube-{}", std::process::id()));
        fs::create_dir_all(home.join(".kube")).unwrap();
        fs::write(home.join(".kube/config"), CONFIG).unwrap();
        let users = with_env(&home, &home, "", |env| complete(env, Kind::User));
        assert_eq!(users.len(), 1);
        let missing = home.join("missing");
        assert!(with_env(&missing, &missing, "", |env| complete(env, Kind::Context)).is_empty());
        fs::remove_dir_all(&home).unwrap();
    }
}
