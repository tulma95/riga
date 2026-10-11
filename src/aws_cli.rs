//! AWS CLI v2 services, operations and flags generated at runtime from the user's installed
//! botocore data (`awscli/botocore/data/<service>/<api-version>/service-2.json`). Nothing is bundled
//! and `aws` is never run: the model files are read like any other value source, bounded and
//! sanitized, and turned into a `Spec` through the same builder as the `.riga` parser.
use crate::{
    background::Runs,
    fsio::{list_dir, read_text},
    json::Json,
    spec::{self, Arg, Builder, FlagDef, Spec, Value, ValueSpec},
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Instant, SystemTime},
};
const MAX_MODEL_BYTES: u64 = 8 << 20;
const MAX_PAGINATORS_BYTES: u64 = 1 << 20;
/// Enough to reach `metadata` in every model; a few put a long `documentation` first and get a
/// generic description instead.
const HEAD_BYTES: u64 = 4096;
const MAX_SERVICES: usize = 2_000;
const MAX_VERSIONS: usize = 256;
const MAX_ENUM: usize = 2_000;
const DESCRIPTION: usize = 100;
const MERGED_CACHE: usize = 16;
const RECHECK: std::time::Duration = std::time::Duration::from_secs(30);
/// Where the CLI's service names differ from botocore's directory names, found by diffing the two
/// lists. ponytail: a service renamed in a later CLI keeps its directory name until this grows.
const RENAMES: [(&str, &str); 5] = [
    ("s3", "s3api"),
    ("codedeploy", "deploy"),
    ("config", "configservice"),
    ("agenttoolkit", "agent-toolkit"),
    ("eventbridgev2", "eventsv2"),
];
/// botocore's `xform_name`: `DescribeDBInstances` becomes `describe-db-instances` for `sep` `-`.
pub fn xform_name(name: &str, sep: char) -> String {
    if name.contains(sep) {
        return name.into();
    }
    // Three names that the two rules below would split wrongly are cached in botocore itself.
    const CACHED: [(&str, &str); 8] = [
        ("CreateCachediSCSIVolume", "create-cached-iscsi-volume"),
        (
            "DescribeCachediSCSIVolumes",
            "describe-cached-iscsi-volumes",
        ),
        (
            "DescribeStorediSCSIVolumes",
            "describe-stored-iscsi-volumes",
        ),
        ("CreateStorediSCSIVolume", "create-stored-iscsi-volume"),
        (
            "ListHITsForQualificationType",
            "list-hits-for-qualification-type",
        ),
        ("ExecutePartiQLStatement", "execute-partiql-statement"),
        ("ExecutePartiQLTransaction", "execute-partiql-transaction"),
        ("ExecutePartiQLBatch", "execute-partiql-batch"),
    ];
    if let Some((_, cached)) = CACHED.iter().find(|(k, _)| *k == name) {
        return cached.replace('-', &sep.to_string());
    }
    let mut chars: Vec<char> = name.chars().collect();
    // `[A-Z]{2,}s$`: a trailing acronym plural (ARNs, IDs) is split off whole.
    if chars.last() == Some(&'s') {
        let upper = chars[..chars.len() - 1]
            .iter()
            .rev()
            .take_while(|c| c.is_ascii_uppercase())
            .count();
        if upper >= 2 {
            let start = chars.len() - 1 - upper;
            let tail: Vec<char> = chars.split_off(start);
            chars.push(sep);
            chars.extend(tail.iter().map(char::to_ascii_lowercase));
        }
    }
    // `(.)([A-Z][a-z]+)` -> `\1sep\2`, scanning left to right without overlap.
    let mut first = vec![];
    let mut i = 0;
    while i < chars.len() {
        let word = chars
            .get(i + 1)
            .is_some_and(char::is_ascii_uppercase)
            .then(|| {
                chars[i + 2..]
                    .iter()
                    .take_while(|c| c.is_ascii_lowercase())
                    .count()
            });
        match word {
            Some(n) if n > 0 => {
                first.extend([chars[i], sep]);
                first.extend(&chars[i + 1..i + 2 + n]);
                i += 2 + n;
            }
            _ => {
                first.push(chars[i]);
                i += 1;
            }
        }
    }
    // `([a-z0-9])([A-Z])` -> `\1sep\2`.
    let mut out = String::new();
    let mut i = 0;
    while i < first.len() {
        let split = (first[i].is_ascii_lowercase() || first[i].is_ascii_digit())
            && first.get(i + 1).is_some_and(char::is_ascii_uppercase);
        out.push(first[i]);
        if split {
            out.push(sep);
            out.push(first[i + 1]);
            i += 1;
        }
        i += 1;
    }
    let out = out.to_lowercase();
    // Partial renames: botocore's `ipv-6`, and brand names that its newer versions join.
    // ponytail: the CLI also keeps an alias for a few (`o-tel`/`otel`); only the main spelling here.
    const JOINED: [(&str, &str); 3] = [
        ("ipv-6", "ipv6"),
        ("lo-ra-wan", "lorawan"),
        ("whats-app", "whatsapp"),
    ];
    JOINED.iter().fold(out, |name, (from, to)| {
        name.replace(&from.replace('-', &sep.to_string()), to)
    })
}
/// Documentation HTML as one line of text.
fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut tag: Option<String> = None;
    let mut entity = String::new();
    for c in html.chars() {
        match (c, &mut tag) {
            ('<', _) => tag = Some(String::new()),
            ('>', Some(name)) => {
                // Block tags separate words; inline ones (`<a>`, `<code>`) do not.
                let name = name.trim_start_matches('/').to_ascii_lowercase();
                let name = name
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .next()
                    .unwrap_or("");
                if !matches!(
                    name,
                    "a" | "code" | "i" | "b" | "em" | "strong" | "member" | "important"
                ) {
                    out.push(' ');
                }
                tag = None;
            }
            (c, Some(name)) => name.push(c),
            _ => plain_char(c, &mut out, &mut entity),
        }
    }
    out.push_str(&entity);
    out
}
/// One character outside a tag, with `entity` collecting `&name;` references.
fn plain_char(c: char, out: &mut String, entity: &mut String) {
    match c {
        '&' => entity.push('&'),
        ';' if !entity.is_empty() => {
            out.push(match entity.as_str() {
                "&amp" => '&',
                "&lt" => '<',
                "&gt" => '>',
                "&quot" => '"',
                "&#39" | "&apos" => '\'',
                _ => ' ',
            });
            entity.clear();
        }
        _ if !entity.is_empty() => {
            entity.push(c);
            // Not an entity after all.
            if entity.len() > 8 {
                out.push_str(entity);
                entity.clear();
            }
        }
        _ => out.push(c),
    }
}
/// The first sentence of documentation HTML, as a spec description.
/// ponytail: cuts at the first ". " that does not follow "e.g"/"i.e"; a few descriptions end early.
fn describe(html: &str) -> String {
    let text = plain(html);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = text.len();
    for (i, _) in text.match_indices(". ") {
        let before = &text[..i];
        if !(before.ends_with("e.g") || before.ends_with("i.e")) {
            end = i + 1;
            break;
        }
    }
    spec::clamp_description(&text[..end], DESCRIPTION)
}
struct Member {
    name: String,
    shape: String,
    description: String,
}
#[derive(Default)]
struct Shape {
    kind: String,
    choices: Vec<String>,
    /// The element shape of a list.
    member: Option<String>,
    /// The member that is the whole response body of an output shape.
    payload: Option<String>,
    eventstream: bool,
    /// Only kept for the input and output shapes of operations.
    members: Vec<Member>,
}
struct Op {
    name: String,
    input: Option<String>,
    output: Option<String>,
    description: String,
}
/// What `service-2.json` says about one service, reduced to what completion needs.
#[derive(Default)]
struct Model {
    title: String,
    ops: Vec<Op>,
    shapes: HashMap<String, Shape>,
}
fn string_member(p: &mut Json, key: &str, wanted: &str) -> Option<Option<String>> {
    if key == wanted {
        p.string_or_skip()
    } else {
        p.skip().map(|()| None)
    }
}
/// Read the `shape` of `{"shape": "Name", ...}`.
fn shape_ref(p: &mut Json) -> Option<Option<String>> {
    let mut found = None;
    p.object(&mut |p, key| {
        if let Some(name) = string_member(p, &key, "shape")? {
            found = Some(name);
        }
        Some(())
    })?;
    Some(found)
}
fn parse_model(text: &str) -> Model {
    let mut model = Model::default();
    // A malformed or cut file keeps whatever was read before the damage.
    let _ = Json::new(text).object(&mut |p, key| match key.as_str() {
        "metadata" => p.object(&mut |p, key| {
            if let Some(title) = string_member(p, &key, "serviceFullName")? {
                model.title = title;
            }
            Some(())
        }),
        "operations" => p.object(&mut |p, name| {
            let mut op = Op {
                name,
                input: None,
                output: None,
                description: String::new(),
            };
            p.object(&mut |p, key| match key.as_str() {
                "input" => {
                    op.input = shape_ref(p)?;
                    Some(())
                }
                "output" => {
                    op.output = shape_ref(p)?;
                    Some(())
                }
                "documentation" => {
                    op.description = describe(&p.string_or_skip()?.unwrap_or_default());
                    Some(())
                }
                _ => p.skip(),
            })?;
            model.ops.push(op);
            Some(())
        }),
        "shapes" => {
            let used: HashSet<String> = model
                .ops
                .iter()
                .flat_map(|o| [o.input.clone(), o.output.clone()])
                .flatten()
                .collect();
            let inputs: HashSet<String> =
                model.ops.iter().filter_map(|o| o.input.clone()).collect();
            p.object(&mut |p, name| {
                let input = inputs.contains(&name);
                let used = used.contains(&name);
                let mut shape = Shape::default();
                p.object(&mut |p, key| match key.as_str() {
                    "type" => {
                        shape.kind = p.string_or_skip()?.unwrap_or_default();
                        Some(())
                    }
                    "enum" => p.array(&mut |p| {
                        let value = p.string_or_skip()?;
                        shape
                            .choices
                            .extend(value.filter(|_| shape.choices.len() < MAX_ENUM));
                        Some(())
                    }),
                    "member" => {
                        shape.member = shape_ref(p)?;
                        Some(())
                    }
                    "payload" => {
                        shape.payload = p.string_or_skip()?;
                        Some(())
                    }
                    "eventstream" => {
                        shape.eventstream = p.literal()? == "true";
                        Some(())
                    }
                    "members" if used => p.object(&mut |p, name| {
                        let mut member = Member {
                            name,
                            shape: String::new(),
                            description: String::new(),
                        };
                        p.object(&mut |p, key| match key.as_str() {
                            "shape" => {
                                member.shape = p.string_or_skip()?.unwrap_or_default();
                                Some(())
                            }
                            "documentation" if input => {
                                member.description =
                                    describe(&p.string_or_skip()?.unwrap_or_default());
                                Some(())
                            }
                            _ => p.skip(),
                        })?;
                        shape.members.push(member);
                        Some(())
                    }),
                    _ => p.skip(),
                })?;
                model.shapes.insert(name, shape);
                Some(())
            })
        }
        _ => p.skip(),
    });
    model
}
/// Operations that `paginators-1.json` lists, and whether each names a `limit_key` (the CLI only
/// offers `--page-size` then).
fn paginated(text: &str) -> HashMap<String, bool> {
    let mut names = HashMap::new();
    let _ = Json::new(text).object(&mut |p, key| {
        if key == "pagination" {
            p.object(&mut |p, name| {
                let mut limit = false;
                p.object(&mut |p, key| {
                    limit |= key == "limit_key";
                    p.skip()
                })?;
                names.insert(name, limit);
                Some(())
            })
        } else {
            p.skip()
        }
    });
    names
}
/// Operations of the API that the CLI removes. ponytail: taken from the CLI's own command list; a
/// later removal stays completable until this grows.
const REMOVED: [(&str, &[&str]); 4] = [
    (
        "ses",
        &[
            "delete-verified-email-address",
            "list-verified-email-addresses",
            "verify-email-address",
        ],
    ),
    ("ec2", &["import-instance", "import-volume"]),
    (
        "emr",
        &[
            "run-job-flow",
            "describe-job-flows",
            "add-job-flow-steps",
            "terminate-job-flows",
            "list-bootstrap-actions",
            "list-instance-groups",
            "set-termination-protection",
            "set-keep-job-flow-alive-when-no-steps",
            "set-visible-to-all-users",
            "set-unhealthy-node-replacement",
        ],
    ),
    ("kinesis", &["subscribe-to-shard"]),
];
fn removed(service: &str, operation: &str) -> bool {
    REMOVED
        .iter()
        .any(|(s, ops)| *s == service && ops.contains(&operation))
}
/// The CLI's own options. A member of an operation that would take one of these names is renamed
/// by the CLI (`--version` becomes `--api-version` or similar) in a per-operation table that is
/// not reproduced here; it is left out instead of shadowing the real option.
/// ponytail: the renamed flag is missing until that table is read from somewhere.
const GLOBAL_OPTIONS: [&str; 19] = [
    "--debug",
    "--endpoint-url",
    "--no-verify-ssl",
    "--no-paginate",
    "--output",
    "--query",
    "--profile",
    "--region",
    "--version",
    "--color",
    "--no-sign-request",
    "--ca-bundle",
    "--cli-read-timeout",
    "--cli-connect-timeout",
    "--cli-binary-format",
    "--no-cli-pager",
    "--cli-auto-prompt",
    "--no-cli-auto-prompt",
    "--cli-error-format",
];
/// Flags that the CLI spells differently from the model, as (service, operation or `*`, generated
/// flag, CLI flag; empty drops it). Mostly the negative form of a boolean (`--no-egress` is
/// `--ingress`), plus a few members renamed away from a clash.
/// ponytail: the CLI's `version` -> `api-version` style renames are not listed; those members are
/// left out (see `GLOBAL_OPTIONS`).
const FLAG_RENAMES: [(&str, &str, &str, &str); 13] = [
    ("ec2", "create-image", "--no-no-reboot", "--reboot"),
    (
        "ec2",
        "create-network-acl-entry",
        "--no-egress",
        "--ingress",
    ),
    (
        "ec2",
        "delete-network-acl-entry",
        "--no-egress",
        "--ingress",
    ),
    (
        "ec2",
        "replace-network-acl-entry",
        "--no-egress",
        "--ingress",
    ),
    (
        "ec2",
        "run-instances",
        "--no-disable-api-termination",
        "--enable-api-termination",
    ),
    ("ec2", "run-instances", "--min-count", ""),
    ("ec2", "run-instances", "--max-count", ""),
    (
        "ecs",
        "*",
        "--no-enable-execute-command",
        "--disable-execute-command",
    ),
    (
        "ecs",
        "execute-command",
        "--no-interactive",
        "--non-interactive",
    ),
    ("sns", "subscribe", "--endpoint", "--notification-endpoint"),
    (
        "workdocs",
        "create-notification-subscription",
        "--endpoint",
        "--notification-endpoint",
    ),
    ("route53domains", "view-billing", "--start", "--start-time"),
    ("route53domains", "view-billing", "--end", "--end-time"),
];
/// Apply `FLAG_RENAMES` to the flags of one operation.
fn rename_flags(service: &str, op: &str, flags: &mut Vec<FlagDef>) {
    for &(s, o, from, to) in &FLAG_RENAMES {
        if s != service || (o != "*" && o != op) {
            continue;
        }
        if to.is_empty() {
            flags.retain(|f| f.names[0] != from);
        }
        for f in flags.iter_mut() {
            for name in f.names.iter_mut().chain(&mut f.excludes) {
                if name == from {
                    *name = to.into();
                }
            }
        }
    }
}
/// An API operation as a subcommand.
pub struct Operation {
    pub name: String,
    pub description: String,
    pub flags: Vec<FlagDef>,
    /// The response body is saved to a file given as the last argument.
    pub outfile: bool,
}
/// Whether a member of the input or output shape of `op` has a shape satisfying `test`; with
/// `payload` only the member that is the whole body counts.
fn has_member(
    model: &Model,
    shape: &Option<String>,
    payload: bool,
    test: fn(&Shape) -> bool,
) -> bool {
    let Some(shape) = shape.as_ref().and_then(|s| model.shapes.get(s)) else {
        return false;
    };
    let mut members = shape
        .members
        .iter()
        .filter(|m| !payload || shape.payload.as_ref() == Some(&m.name));
    members.any(|m| model.shapes.get(&m.shape).is_some_and(test))
}
/// Event stream operations are not commands of the CLI.
fn event_stream(model: &Model, op: &Op) -> bool {
    let events = |s: &Shape| s.eventstream;
    has_member(model, &op.output, false, events) || has_member(model, &op.input, false, events)
}
fn file() -> Option<ValueSpec> {
    Some(ValueSpec {
        value: Value::File,
        optional: false,
    })
}
fn text(label: &str) -> Option<ValueSpec> {
    Some(ValueSpec {
        value: Value::Text(label.into()),
        optional: false,
    })
}
fn flag(name: &str, value: Option<ValueSpec>, description: &str) -> FlagDef {
    FlagDef {
        names: vec![name.into()],
        value,
        description: description.into(),
        ..FlagDef::default()
    }
}
/// The flag for one input member; booleans also get `--no-name`.
fn member_flags(model: &Model, member: &Member) -> Vec<FlagDef> {
    let name = format!("--{}", xform_name(&member.name, '-'));
    let shape = model.shapes.get(&member.shape);
    let kind = shape.map_or("string", |s| s.kind.as_str());
    let mut description = member.description.clone();
    let mut def = flag(&name, None, "");
    let mut flags = vec![];
    let choices = |shape: Option<&Shape>| {
        shape.filter(|s| !s.choices.is_empty()).map(|s| ValueSpec {
            value: Value::Choices(s.choices.clone()),
            optional: false,
        })
    };
    match kind {
        "boolean" => {
            let negated = format!("--no-{}", &name[2..]);
            flags.push(flag(&negated, None, &format!("Do not set {name}")));
            def.excludes.push(negated);
        }
        "list" => {
            def.repeat = true;
            let item = shape
                .and_then(|s| s.member.as_ref())
                .and_then(|m| model.shapes.get(m));
            def.value = choices(item).or_else(|| text("value"));
        }
        "structure" | "map" => def.value = text("json"),
        "integer" | "long" => def.value = text("integer"),
        "double" | "float" => def.value = text("number"),
        "timestamp" => def.value = text("timestamp"),
        "blob" => def.value = file(),
        _ => def.value = choices(shape).or_else(|| text("string")),
    }
    if description.is_empty() {
        description = format!("{} value", kind.replace("structure", "JSON"));
    }
    def.description = description;
    flags.push(def);
    flags
}
/// Every operation of the model with its flags: the input members, then what the CLI adds.
fn operations(service: &str, model: &Model, paginated: &HashMap<String, bool>) -> Vec<Operation> {
    let skeleton = ValueSpec {
        value: Value::Choices(vec!["input".into(), "output".into(), "yaml-input".into()]),
        optional: true,
    };
    let mut seen = HashSet::new();
    let mut out = vec![];
    for op in &model.ops {
        let name = xform_name(&op.name, '-');
        if event_stream(model, op) || removed(service, &name) || !seen.insert(name.clone()) {
            continue;
        }
        let input = op.input.as_ref().and_then(|i| model.shapes.get(i));
        let mut flags: Vec<FlagDef> = input
            .iter()
            .flat_map(|s| &s.members)
            .flat_map(|m| member_flags(model, m))
            .filter(|f| !GLOBAL_OPTIONS.contains(&f.names[0].as_str()))
            .collect();
        rename_flags(service, &name, &mut flags);
        // An operation whose response is a blob body saves it to a file argument and lacks the
        // input-file flags.
        let outfile = has_member(model, &op.output, true, |s| s.kind == "blob");
        if !outfile {
            flags.push(flag(
                "--cli-input-json",
                file(),
                "Read arguments from a JSON string or file",
            ));
            flags.push(flag(
                "--cli-input-yaml",
                file(),
                "Read arguments from a YAML string or file",
            ));
            flags.push(flag(
                "--generate-cli-skeleton",
                Some(skeleton.clone()),
                "Print a skeleton of the arguments",
            ));
        }
        if let Some(&limit) = paginated.get(&op.name) {
            flags.push(flag(
                "--starting-token",
                text("token"),
                "Token to resume pagination from",
            ));
            if limit {
                flags.push(flag(
                    "--page-size",
                    text("integer"),
                    "Items to request per API call",
                ));
            }
            flags.push(flag(
                "--max-items",
                text("integer"),
                "Total items to return",
            ));
        }
        let description = if op.description.is_empty() {
            format!("Call {}", op.name)
        } else {
            op.description.clone()
        };
        out.push(Operation {
            name,
            description,
            flags,
            outfile,
        });
    }
    out
}
/// A service directory and the name the CLI gives it.
#[derive(Clone, Debug)]
pub struct Service {
    pub name: String,
    /// The newest `<api-version>` directory.
    pub dir: PathBuf,
}
/// Services under a botocore data directory, sorted by CLI name.
pub fn services(data: &Path) -> Vec<Service> {
    let mut out = vec![];
    for entry in list_dir(data, MAX_SERVICES).unwrap_or_default() {
        if !entry.file_type.is_dir() {
            continue;
        }
        let versions = list_dir(&data.join(&entry.name), MAX_VERSIONS).unwrap_or_default();
        // API versions are dates, so the last name is the newest.
        let newest = versions.iter().rev().find_map(|v| {
            let dir = data.join(&entry.name).join(&v.name);
            dir.join("service-2.json").is_file().then_some(dir)
        });
        let name = RENAMES
            .iter()
            .find(|(dir, _)| *dir == entry.name)
            .map_or(entry.name.as_str(), |&(_, cli)| cli);
        if let Some(dir) = newest {
            out.push(Service {
                name: name.into(),
                dir,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
/// The operations of a service, or none when its model cannot be read.
pub fn service_operations(service: &Service) -> (String, Vec<Operation>) {
    let Some(text) = read_text(&service.dir.join("service-2.json"), MAX_MODEL_BYTES) else {
        return (String::new(), vec![]);
    };
    let model = parse_model(&text);
    let pages = read_text(&service.dir.join("paginators-1.json"), MAX_PAGINATORS_BYTES)
        .map(|t| paginated(&t))
        .unwrap_or_default();
    (
        model.title.clone(),
        operations(&service.name, &model, &pages),
    )
}
/// A service's full name from the head of its model, for the service list.
fn title(service: &Service) -> String {
    let head = read_text(&service.dir.join("service-2.json"), HEAD_BYTES).unwrap_or_default();
    let model = parse_model(&head);
    if model.title.is_empty() {
        "AWS service".into()
    } else {
        spec::clamp_description(&model.title, DESCRIPTION)
    }
}
/// The botocore data directory next to the `aws` on `path`.
pub fn locate(path: &str) -> Option<PathBuf> {
    resolve(path).map(|(data, _)| data)
}
/// The data directory and the real `aws` executable it was found from.
fn resolve(path: &str) -> Option<(PathBuf, PathBuf)> {
    let exe = std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("aws"))
        .find(|exe| {
            fs::metadata(exe).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })?;
    // Symlinks lead from /usr/local/bin or the Homebrew bin to the versioned install.
    let real = fs::canonicalize(exe).ok()?;
    let data = real.ancestors().skip(1).take(6).find_map(data_in)?;
    Some((data, real))
}
/// Modification time as a stamp component.
fn mtime(path: &Path) -> u128 {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok();
    modified
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}
/// `awscli/botocore/data` below `dir` in the official layout (`.../dist`) or Homebrew's
/// (`libexec/lib/python3.*/site-packages`).
fn data_in(dir: &Path) -> Option<PathBuf> {
    const TAIL: &str = "awscli/botocore/data";
    let mut candidates = vec![dir.join(TAIL), dir.join("dist").join(TAIL)];
    for lib in ["lib", "libexec/lib"] {
        let pythons = list_dir(&dir.join(lib), 64).unwrap_or_default();
        for e in pythons.iter().filter(|e| e.name.starts_with("python3")) {
            candidates.push(dir.join(lib).join(&e.name).join("site-packages").join(TAIL));
        }
    }
    candidates.into_iter().find(|c| c.is_dir())
}
/// The first argument of the background runs that build specs; `background::Runs` hands them to
/// `Cli::exec`.
pub const RUN: &str = "riga-aws-cli";
/// Builds are requested only while their result is missing, so a finished run is never reused.
const TTL: std::time::Duration = std::time::Duration::ZERO;
struct Root {
    stamp: String,
    spec: Arc<Spec>,
    services: Vec<Service>,
}
#[derive(Default)]
struct Shared {
    path: String,
    checked: Option<Instant>,
    /// (stamp, directory): the data found for `path`. The stamp changes with the directory's mtime,
    /// so a reinstall or a new CLI version is picked up.
    data: Option<(String, PathBuf)>,
    curated: Option<Arc<Spec>>,
    root: Option<Root>,
    /// (stamp, service, spec), least recently used first.
    merged: Vec<(String, String, Arc<Spec>)>,
}
/// The answer to a request for a generated spec.
pub enum Lookup {
    /// There is nothing to generate: no CLI data, or the service is not a generated one.
    Absent,
    /// The data exists and is being read in the background.
    Pending,
    Ready(Arc<Spec>),
}
/// Merged `aws` specs for the installed CLI's data. Reading the models takes tens of milliseconds
/// (`ec2` alone is 4 MiB), so builds run on background threads through `background::Runs`; until
/// one is done the caller falls back to the curated spec.
#[derive(Clone, Default)]
pub struct Cli {
    shared: Arc<Mutex<Shared>>,
}
impl Cli {
    /// The build handler for `Runs::with_custom`; `None` for runs that are not ours.
    pub fn exec(&self) -> impl Fn(&str, &[String]) -> Option<Option<Vec<String>>> + use<> {
        let shared = self.shared.clone();
        move |_, argv| match argv {
            [run, kind, stamp, dir, rest @ ..] if run == RUN => {
                Some(build(&shared, kind, stamp, Path::new(dir), rest).map(|()| vec![]))
            }
            _ => None,
        }
    }
    /// Re-resolve `aws` when PATH changed or the last check is old, and return the data found.
    fn data(&self, path: &str, curated: &Spec) -> Option<(String, PathBuf)> {
        let mut s = self.shared.lock().unwrap();
        if s.path != path || s.checked.is_none_or(|t| t.elapsed() >= RECHECK) {
            s.path = path.into();
            s.checked = Some(Instant::now());
            // The stamp changes with the data directory or the executable, so an upgrade in place
            // is noticed. ponytail: a data file edited in place under an old mtime is not.
            s.data = resolve(path).map(|(dir, exe)| {
                let stamp = format!(
                    "{}:{}:{}:{}",
                    mtime(&dir),
                    mtime(&exe),
                    exe.display(),
                    dir.display()
                );
                (stamp, dir)
            });
        }
        let data = s.data.clone()?;
        if s.root.as_ref().is_some_and(|r| r.stamp != data.0) {
            s.root = None;
            s.merged.clear();
        }
        s.curated.get_or_insert_with(|| Arc::new(curated.clone()));
        Some(data)
    }
    /// The stamp of the install found on `path`, for tests.
    #[cfg(test)]
    pub(crate) fn stamp(&self, path: &str, curated: &Spec) -> Option<String> {
        self.data(path, curated).map(|d| d.0)
    }
    /// The curated spec plus a command per installed service.
    pub fn root(&self, runs: &Runs, path: &str, curated: &Spec) -> Lookup {
        let Some((stamp, dir)) = self.data(path, curated) else {
            return Lookup::Absent;
        };
        if let Some(root) = self
            .shared
            .lock()
            .unwrap()
            .root
            .as_ref()
            .filter(|r| r.stamp == stamp)
        {
            return Lookup::Ready(root.spec.clone());
        }
        runs.get(path, &[RUN, "root", &stamp, &dir.to_string_lossy()], TTL);
        Lookup::Pending
    }
    /// The root spec with the operations of `service` added. `Absent` for a name that is not a
    /// generated service (the root already knows what to do with it).
    pub fn service(&self, runs: &Runs, path: &str, curated: &Spec, service: &str) -> Lookup {
        let Some((stamp, dir)) = self.data(path, curated) else {
            return Lookup::Absent;
        };
        {
            let mut s = self.shared.lock().unwrap();
            let Some(root) = s.root.as_ref().filter(|r| r.stamp == stamp) else {
                return Lookup::Pending;
            };
            if !root.services.iter().any(|v| v.name == service) {
                return Lookup::Absent;
            }
            if let Some(i) = s
                .merged
                .iter()
                .position(|(t, n, _)| *t == stamp && n == service)
            {
                let hit = s.merged.remove(i);
                let spec = hit.2.clone();
                s.merged.push(hit);
                return Lookup::Ready(spec);
            }
        }
        runs.get(
            path,
            &[RUN, "service", &stamp, &dir.to_string_lossy(), service],
            TTL,
        );
        Lookup::Pending
    }
}
/// Runs on a background thread. `None` when there was nothing to build.
fn build(
    shared: &Mutex<Shared>,
    kind: &str,
    stamp: &str,
    dir: &Path,
    rest: &[String],
) -> Option<()> {
    let curated = shared.lock().unwrap().curated.clone()?;
    if kind == "root" {
        let (services, spec) = root_spec(&curated, &services(dir))?;
        let spec = Arc::new(spec);
        shared.lock().unwrap().root = Some(Root {
            stamp: stamp.into(),
            spec,
            services,
        });
        return Some(());
    }
    let name = rest.first()?;
    let (root, service) = {
        let s = shared.lock().unwrap();
        let root = s.root.as_ref().filter(|r| r.stamp == stamp)?;
        (
            root.spec.clone(),
            root.services.iter().find(|v| v.name == *name)?.clone(),
        )
    };
    let spec = Arc::new(merge(&root, &service)?);
    let mut s = shared.lock().unwrap();
    // A newer directory may have replaced the root meanwhile.
    if s.root.as_ref().is_none_or(|r| r.stamp != stamp) {
        return None;
    }
    if s.merged.len() >= MERGED_CACHE {
        s.merged.remove(0);
    }
    s.merged.push((stamp.into(), name.clone(), spec));
    Some(())
}
/// The curated `aws` spec with every installed service as a subcommand. Its root is no longer
/// open, since the real service names are known.
fn root_spec(curated: &Spec, services: &[Service]) -> Option<(Vec<Service>, Spec)> {
    if services.is_empty() {
        return None;
    }
    let mut b = Builder::from_spec(curated.clone(), "aws");
    b.set_open(0, false);
    for s in services {
        if !b.spec().commands[0].child_names.contains_key(&s.name) {
            let _ = b.command(Some(0), vec![s.name.clone()], false, false, title(s));
        }
    }
    Some((services.to_vec(), b.finish().ok()?))
}
/// `root` with the operations of one service. A curated operation keeps its own flags, arguments
/// and subcommands and gains the generated flags it lacks.
fn merge(root: &Spec, service: &Service) -> Option<Spec> {
    let (_, ops) = service_operations(service);
    let mut b = Builder::from_spec(root.clone(), "aws");
    let parent = *b.spec().commands[0].child_names.get(&service.name)?;
    for op in ops {
        let existing = b.spec().commands[parent].child_names.get(&op.name).copied();
        // An operation whose name or description the builder refuses is left out.
        let Some(command) = existing.or_else(|| {
            b.command(Some(parent), vec![op.name], false, false, op.description)
                .ok()
        }) else {
            continue;
        };
        if op.outfile && b.spec().commands[command].args.is_empty() {
            let outfile = Arg {
                value: ValueSpec {
                    value: Value::File,
                    optional: false,
                },
                repeat: false,
                stop: false,
                description: "File to save the response body to".into(),
            };
            let _ = b.arg(command, outfile);
        }
        for mut flag in op.flags {
            // `--no-x` comes first; if it was refused, `--x` has nothing to exclude. A flag the
            // curated command already spells is refused as a duplicate, so the curated one wins.
            let own = &b.spec().commands[command].flag_names;
            flag.excludes.retain(|n| own.contains_key(n));
            let _ = b.flag(command, 0, flag);
        }
    }
    b.finish().ok()
}
/// A fake CLI install in a temporary directory: `aws` on the returned PATH resolves through a symlink
/// to `.../dist/aws`, next to `dist/awscli/botocore/data` with tiny models.
#[cfg(test)]
pub(crate) fn fake_install(tag: &str) -> (PathBuf, String) {
    let root = std::env::temp_dir().join(format!("riga-awscli-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let dist = root.join("aws-cli/v2/2.0.0/dist");
    let data = dist.join("awscli/botocore/data");
    let write = |rel: &str, text: &str| {
        let file = data.join(rel);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    };
    let operation = |name: &str, doc: &str| {
        format!(
            r#""{name}": {{"name": "{name}", "input": {{"shape": "{name}Request"}}, "documentation": "{doc}"}}"#
        )
    };
    let model = |title: &str, operations: &str, shapes: &str| {
        format!(
            r#"{{"version": "2.0", "metadata": {{"apiVersion": "1", "serviceFullName": "{title}"}},
            "operations": {{{operations}}},
            "shapes": {{{shapes}, "Boolean": {{"type": "boolean"}}, "Integer": {{"type": "integer"}},
            "String": {{"type": "string"}}, "Blob": {{"type": "blob"}}, "Strings": {{"type": "list", "member": {{"shape": "String"}}}}}},
            "documentation": "x"}}"#
        )
    };
    let ec2_ops = [
        operation("DescribeInstances", "Describes instances."),
        operation("RunInstances", "Curated wins."),
        operation(
            "DescribeHosts",
            "<p>Describes the &quot;Dedicated&quot; hosts, e.g. all. Not this.</p>",
        ),
        operation("CreateImage", "Creates an image."),
    ]
    .join(",");
    let ec2_shapes = r#""DescribeInstancesRequest": {"type": "structure", "members": {}},
        "RunInstancesRequest": {"type": "structure", "members": {"MinCount": {"shape": "Integer"}}},
        "DescribeHostsRequest": {"type": "structure", "members": {
          "HostIds": {"shape": "Strings", "documentation": "<p>The host IDs.</p>"},
          "DryRun": {"shape": "Boolean", "documentation": "<p>Check permissions.</p>"},
          "MaxResults": {"shape": "Integer"},
          "Query": {"shape": "String"},
          "HostRecovery": {"shape": "HostRecovery", "documentation": "<p>Recovery mode.\u202e</p>"},
          "Ipv6AddressCount": {"shape": "Integer"}}},
        "CreateImageRequest": {"type": "structure", "members": {
          "NoReboot": {"shape": "Boolean"}, "Name": {"shape": "String"}}},
        "HostRecovery": {"type": "string", "enum": ["on", "off"]}"#;
    write(
        "ec2/2016-11-15/service-2.json",
        &model("Amazon Elastic Compute Cloud", &ec2_ops, ec2_shapes),
    );
    write(
        "ec2/2016-11-15/paginators-1.json",
        r#"{"pagination": {"DescribeHosts": {"input_token": "NextToken", "limit_key": "MaxResults"}}}"#,
    );
    write(
        "ec2/2015-01-01/service-2.json",
        &model(
            "Old",
            &operation("OldOperation", "Gone."),
            r#""OldOperationRequest": {"type": "structure", "members": {}}"#,
        ),
    );
    let rds_ops = [
        operation("DescribeDBInstances", "Lists databases."),
        operation("ListACLs", "Lists ACLs."),
    ]
    .join(",");
    let rds_shapes = r#""DescribeDBInstancesRequest": {"type": "structure", "members": {
          "DBInstanceIdentifier": {"shape": "String"}, "Data": {"shape": "Blob"}}},
        "ListACLsRequest": {"type": "structure", "members": {}}"#;
    write(
        "rds/2014-10-31/service-2.json",
        &model("Amazon Relational Database Service", &rds_ops, rds_shapes),
    );
    let sns_shapes = r#""SubscribeRequest": {"type": "structure", "members": {
          "Endpoint": {"shape": "String"}, "TopicArn": {"shape": "String"}}}"#;
    write(
        "sns/2010-03-31/service-2.json",
        &model(
            "Amazon SNS",
            &operation("Subscribe", "Subscribes."),
            sns_shapes,
        ),
    );
    // The CLI names this directory `s3api`; the curated spec already has `list-buckets`.
    let s3_ops = [
        operation("ListBuckets", "Curated wins."),
        operation("GetBucketFoo", "Gets a foo."),
    ]
    .join(",");
    let s3_shapes = r#""ListBucketsRequest": {"type": "structure", "members": {}},
        "GetBucketFooRequest": {"type": "structure", "members": {}}"#;
    write(
        "s3/2006-03-01/service-2.json",
        &model("Amazon S3", &s3_ops, s3_shapes),
    );
    write("endpoints.json", "{}");
    fs::write(dist.join("aws"), "").unwrap();
    fs::set_permissions(dist.join("aws"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    std::os::unix::fs::symlink(dist.join("aws"), root.join("bin/aws")).unwrap();
    let path = root.join("bin").to_string_lossy().into_owned();
    (root, path)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_follow_botocore_xform_name() {
        for (name, kebab) in [
            ("DescribeInstances", "describe-instances"),
            ("DescribeDBInstances", "describe-db-instances"),
            ("DBInstanceIdentifier", "db-instance-identifier"),
            ("KMSKeyId", "kms-key-id"),
            ("Ipv6AddressCount", "ipv6-address-count"),
            ("ListARNs", "list-arns"),
            ("ListACLs", "list-acls"),
            ("SourceServerIDs", "source-server-ids"),
            ("MemorySizeInGBs", "memory-size-in-gbs"),
            ("AllowedOAuthFlows", "allowed-o-auth-flows"),
            ("LoRaWAN", "lorawan"),
            ("CreateCachediSCSIVolume", "create-cached-iscsi-volume"),
            (
                "ListHITsForQualificationType",
                "list-hits-for-qualification-type",
            ),
            ("ExecutePartiQLBatch", "execute-partiql-batch"),
            ("S3Uri2", "s3-uri2"),
            ("already-kebab", "already-kebab"),
            ("", ""),
        ] {
            assert_eq!(xform_name(name, '-'), kebab, "{name}");
        }
        assert_eq!(
            xform_name("DescribeDBInstances", '_'),
            "describe_db_instances"
        );
    }
    #[test]
    fn documentation_becomes_one_short_sentence() {
        assert_eq!(
            describe(
                "<p>Describes the &quot;Dedicated&quot; hosts, e.g. all &amp; more. Second.</p>"
            ),
            "Describes the \"Dedicated\" hosts, e.g. all & more."
        );
        assert_eq!(
            describe("<p>No <i>period</i></p>\n<ul><li>x</li></ul>"),
            "No period x"
        );
        let long = describe(&format!("<p>{}</p>", "word ".repeat(60)));
        assert!(
            long.chars().count() <= DESCRIPTION && long.ends_with('…'),
            "{long}"
        );
        assert_eq!(describe("evil\u{202e}text\u{1b}"), "eviltext");
    }
    fn cli(operations: &[Operation], name: &str) -> Vec<(String, bool, bool)> {
        let op = operations.iter().find(|o| o.name == name).unwrap();
        op.flags
            .iter()
            .map(|f| (f.names[0].clone(), f.repeat, f.value.is_none()))
            .collect()
    }
    #[test]
    fn models_become_operations_and_flags() {
        let (root, path) = fake_install("model");
        let data = locate(&path).expect("data next to the symlinked aws");
        assert!(data.ends_with("dist/awscli/botocore/data"));
        let all = services(&data);
        let names: Vec<_> = all.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["ec2", "rds", "s3api", "sns"]);
        assert!(all[0].dir.ends_with("ec2/2016-11-15"));
        let (title, ec2) = service_operations(&all[0]);
        assert_eq!(title, "Amazon Elastic Compute Cloud");
        let ops: Vec<_> = ec2.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            ops,
            [
                "describe-instances",
                "run-instances",
                "describe-hosts",
                "create-image"
            ]
        );
        assert_eq!(
            ec2[2].description,
            "Describes the \"Dedicated\" hosts, e.g. all."
        );
        let flags = cli(&ec2, "describe-hosts");
        let names: Vec<_> = flags.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(
            names,
            [
                "--host-ids",
                "--no-dry-run",
                "--dry-run",
                "--max-results",
                "--host-recovery",
                "--ipv6-address-count",
                "--cli-input-json",
                "--cli-input-yaml",
                "--generate-cli-skeleton",
                "--starting-token",
                "--page-size",
                "--max-items",
            ]
        );
        assert!(flags[0].1, "lists repeat");
        assert!(flags[2].2 && flags[1].2, "booleans take no value");
        assert!(
            !names.contains(&"--query"),
            "a member named like a global option is left out"
        );
        // The CLI spells the negated `NoReboot` as `--reboot`, and renames `--endpoint` of sns.
        let image: Vec<_> = ec2[3].flags.iter().map(|f| f.names[0].as_str()).collect();
        assert_eq!(image[..3], ["--reboot", "--no-reboot", "--name"]);
        assert_eq!(ec2[3].flags[1].excludes, ["--reboot"]);
        let (_, sns) = service_operations(&all[3]);
        assert_eq!(sns[0].flags[0].names[0], "--notification-endpoint");
        // Blobs and the input files are paths.
        let (_, rds) = service_operations(&all[1]);
        let is_file = |f: &FlagDef| {
            matches!(
                &f.value,
                Some(ValueSpec {
                    value: Value::File,
                    ..
                })
            )
        };
        let by = |name: &str| rds[0].flags.iter().find(|f| f.names[0] == name).unwrap();
        assert!(is_file(by("--data")) && is_file(by("--cli-input-json")));
        assert!(is_file(by("--cli-input-yaml")) && !is_file(by("--db-instance-identifier")));
        let hosts = &ec2[2].flags;
        assert_eq!(hosts[2].excludes, ["--no-dry-run"]);
        assert_eq!(hosts[4].description, "Recovery mode.");
        assert_eq!(hosts[0].description, "The host IDs.");
        assert_eq!(hosts[3].description, "integer value");
        assert!(
            matches!(&hosts[4].value, Some(ValueSpec { value: Value::Choices(c), .. }) if c == &["on", "off"])
        );
        assert!(
            cli(&ec2, "describe-instances")
                .iter()
                .all(|f| f.0 != "--starting-token")
        );
        let (_, rds) = service_operations(&all[1]);
        let ops: Vec<_> = rds.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(ops, ["describe-db-instances", "list-acls"]);
        assert_eq!(
            cli(&rds, "describe-db-instances")[0].0,
            "--db-instance-identifier"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn hostile_files_are_bounded_or_ignored() {
        let (root, path) = fake_install("hostile");
        let data = locate(&path).unwrap();
        // Truncated, binary and oversized models keep what was readable and never panic.
        let service = Service {
            name: "x".into(),
            dir: data.join("ec2/2016-11-15"),
        };
        let file = service.dir.join("service-2.json");
        let good = fs::read_to_string(&file).unwrap();
        for cut in [10, good.len() / 2, good.len() - 3] {
            fs::write(&file, &good[..cut]).unwrap();
            let _ = service_operations(&service);
        }
        fs::write(&file, vec![0xffu8; 100]).unwrap();
        assert!(service_operations(&service).1.is_empty());
        fs::write(&file, " ".repeat(MAX_MODEL_BYTES as usize + 10) + &good).unwrap();
        assert!(service_operations(&service).1.is_empty());
        fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink("/dev/null", &file).unwrap();
        assert!(service_operations(&service).1.is_empty());
        let _ = (&file, &data);
        // `aws` that is not an executable file is not found.
        fs::remove_dir_all(root.join("aws-cli")).unwrap();
        assert!(locate(&path).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    /// Dev check against the CLI's own autocomplete index (a SQLite file), via the `sqlite3` tool:
    /// `RIGA_AWS_ORACLE=<dir with botocore/ and clidata/ac.index> cargo test oracle -- --nocapture`.
    #[test]
    fn generated_names_match_the_cli_autocomplete_index() {
        let Some(root) = std::env::var_os("RIGA_AWS_ORACLE").map(PathBuf::from) else {
            return;
        };
        let sql = |query: &str| -> Vec<Vec<String>> {
            let out = std::process::Command::new("sqlite3")
                .arg("-separator")
                .arg("\t")
                .arg(root.join("clidata/ac.index"))
                .arg(query)
                .output()
                .expect("sqlite3");
            String::from_utf8(out.stdout)
                .unwrap()
                .lines()
                .map(|l| l.split('\t').map(str::to_owned).collect())
                .collect()
        };
        let index_services: HashSet<String> =
            sql("select command from command_table where parent = 'aws'")
                .into_iter()
                .map(|r| r[0].clone())
                .collect();
        let mut index_ops: HashMap<String, HashSet<String>> = HashMap::new();
        for r in sql(
            "select parent, command from command_table where parent like 'aws.%' and parent not like 'aws.%.%'",
        ) {
            index_ops
                .entry(r[0].trim_start_matches("aws.").into())
                .or_default()
                .insert(r[1].clone());
        }
        let mut index_flags: HashMap<(String, String), HashSet<String>> = HashMap::new();
        for r in sql(
            "select parent, command, argname from param_table where parent like 'aws.%' and (positional_arg is null or positional_arg = '0') and parent not like 'aws.%.%'",
        ) {
            index_flags
                .entry((r[0].trim_start_matches("aws.").into(), r[1].clone()))
                .or_default()
                .insert(r[2].clone());
        }
        let all = services(&root.join("botocore"));
        let generated: HashSet<String> = all.iter().map(|s| s.name.clone()).collect();
        let (mut ops_total, mut ops_hit, mut ops_exact) = (0, 0, 0);
        let (mut flag_total, mut flag_hit, mut index_flag_total, mut index_flag_hit) = (0, 0, 0, 0);
        let (mut only_index_ops, mut examples, mut extra_flags, mut missing_flags) = (
            vec![],
            vec![],
            HashMap::<String, usize>::new(),
            HashMap::<String, usize>::new(),
        );
        for service in &all {
            let (_, ops) = service_operations(service);
            let known = index_ops.get(&service.name).cloned().unwrap_or_default();
            let mut seen = HashSet::new();
            for op in &ops {
                ops_total += 1;
                seen.insert(op.name.clone());
                if !known.contains(&op.name) {
                    examples.push(format!("op {} {} not in index", service.name, op.name));
                    continue;
                }
                ops_hit += 1;
                let mut ours: HashSet<String> = op
                    .flags
                    .iter()
                    .map(|f| f.names[0][2..].to_owned())
                    .collect();
                if op.outfile {
                    ours.insert("outfile".into());
                }
                let theirs = index_flags
                    .get(&(service.name.clone(), op.name.clone()))
                    .cloned()
                    .unwrap_or_default();
                flag_total += ours.len();
                index_flag_total += theirs.len();
                flag_hit += ours.intersection(&theirs).count();
                index_flag_hit += ours.intersection(&theirs).count();
                if ours == theirs {
                    ops_exact += 1;
                } else {
                    for f in ours.difference(&theirs) {
                        *extra_flags.entry(f.clone()).or_default() += 1;
                    }
                    for f in theirs.difference(&ours) {
                        *missing_flags.entry(f.clone()).or_default() += 1;
                    }
                    if examples.len() < 40 {
                        let mut extra: Vec<_> = ours.difference(&theirs).cloned().collect();
                        let mut gone: Vec<_> = theirs.difference(&ours).cloned().collect();
                        extra.sort();
                        gone.sort();
                        examples.push(format!(
                            "flags {} {}: generated-only {extra:?} index-only {gone:?}",
                            service.name, op.name
                        ));
                    }
                }
            }
            for name in known.difference(&seen) {
                only_index_ops.push(format!("{} {name}", service.name));
            }
        }
        let pct = |a: usize, b: usize| 100.0 * a as f64 / b.max(1) as f64;
        let top = |m: &HashMap<String, usize>| {
            let mut v: Vec<_> = m.iter().collect();
            v.sort_by_key(|(k, n)| (std::cmp::Reverse(**n), (*k).clone()));
            v.into_iter()
                .take(12)
                .map(|(k, n)| format!("{k}:{n}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let missing_services: Vec<_> = {
            let mut v: Vec<_> = index_services.difference(&generated).cloned().collect();
            v.sort();
            v
        };
        let extra_services: Vec<_> = generated.difference(&index_services).collect();
        println!(
            "services: generated {} of {} in index; generated-not-in-index {extra_services:?}; index-only (customizations) {missing_services:?}",
            generated.len() - extra_services.len(),
            index_services.len()
        );
        assert!(pct(ops_hit, ops_total) >= 99.9, "operation names regressed");
        assert!(pct(flag_hit, flag_total) >= 99.9, "flag names regressed");
        println!(
            "service names matched: {:.2}%",
            pct(generated.len() - extra_services.len(), generated.len())
        );
        println!(
            "operations: {ops_hit}/{ops_total} generated names in index = {:.2}%",
            pct(ops_hit, ops_total)
        );
        let wait_or_custom = only_index_ops
            .iter()
            .filter(|o| !o.ends_with(" wait") && !o.ends_with(" help"))
            .count();
        println!(
            "index operations we do not generate: {} (of which not wait/help: {wait_or_custom})",
            only_index_ops.len()
        );
        println!(
            "flags of matched operations: {:.2}% of generated in index ({flag_hit}/{flag_total}), {:.2}% of index covered ({index_flag_hit}/{index_flag_total}), operations with identical flag sets {:.2}% ({ops_exact}/{ops_hit})",
            pct(flag_hit, flag_total),
            pct(index_flag_hit, index_flag_total),
            pct(ops_exact, ops_hit)
        );
        println!("most common generated-only flags: {}", top(&extra_flags));
        println!("most common index-only flags: {}", top(&missing_flags));
        for line in examples.iter().take(40) {
            println!("  {line}");
        }
        for line in only_index_ops
            .iter()
            .filter(|o| !o.ends_with(" wait"))
            .take(25)
        {
            println!("  index-only op {line}");
        }
    }
}
