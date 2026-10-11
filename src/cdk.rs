//! AWS CDK stack names read from the cloud assembly of the last synth (`cdk.out/manifest.json` in
//! the working directory); never runs `cdk`, which would execute the app's code.
use crate::{
    fsio::read_text,
    json::Json,
    names::{Names, Ref},
    sources::Env,
};
const MAX_BYTES: u64 = 8 << 20;
const MAX_NAMES: usize = 5_000;
/// ponytail: only the default `cdk.out` of the top-level assembly; `--output`/`cdk.json` output
/// settings and nested stage assemblies (`assembly-*/manifest.json`) are not followed.
pub fn stacks(env: &Env) -> Vec<Ref> {
    read_text(&env.cwd.join("cdk.out/manifest.json"), MAX_BYTES)
        .map(|text| parse(&text))
        .unwrap_or_default()
}
/// `artifacts` entries of type `aws:cloudformation:stack`, named the way the CLI selects them: the
/// `displayName` (hierarchical id) when present, else the artifact id.
fn parse(text: &str) -> Vec<Ref> {
    let mut names = Names::new(MAX_NAMES);
    let _ = Json::new(text).object(&mut |p, key| {
        if key != "artifacts" || !p.at_object() {
            return p.skip();
        }
        p.object(&mut |p, id| {
            if !p.at_object() {
                return p.skip();
            }
            let (mut kind, mut display, mut stack_name) = (None, None, None);
            p.object(&mut |p, field| match field.as_str() {
                "type" => p.string_or_skip().map(|s| kind = s),
                "displayName" => p.string_or_skip().map(|s| display = s),
                "properties" if p.at_object() => p.object(&mut |p, prop| {
                    if prop == "stackName" {
                        p.string_or_skip().map(|s| stack_name = s)
                    } else {
                        p.skip()
                    }
                }),
                _ => p.skip(),
            })?;
            if kind.as_deref() == Some("aws:cloudformation:stack") {
                let description =
                    stack_name.map_or("CDK stack".into(), |s| format!("CloudFormation stack {s}"));
                names.push(display.as_deref().unwrap_or(&id), &description);
            }
            Some(())
        })
    });
    names.finish()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stacks_come_from_the_manifest_artifacts() {
        let manifest = r#"{"version": "36.0.0", "artifacts": {
            "Tree": {"type": "cdk:tree", "properties": {"file": "tree.json"}},
            "ApiStack": {"type": "aws:cloudformation:stack", "environment": "aws://unknown-account/unknown-region",
                "properties": {"templateFile": "ApiStack.template.json", "stackName": "prod-api"},
                "metadata": {"/ApiStack/Fn": [{"type": "aws:cdk:logicalId", "data": "Fn"}]},
                "displayName": "ApiStack"},
            "DbStack": {"type": "aws:cloudformation:stack", "properties": {"templateFile": "DbStack.template.json"}},
            "ApiStack.assets": {"type": "cdk:asset-manifest"},
            "Bad": {"type": "aws:cloudformation:stack", "displayName": "a\u202eb"}
        }, "missing": "#;
        let got: Vec<_> = parse(manifest)
            .into_iter()
            .map(|r| (r.name, r.description))
            .collect();
        assert_eq!(
            got,
            [
                ("ApiStack".into(), "CloudFormation stack prod-api".into()),
                ("DbStack".into(), "CDK stack".into())
            ]
        );
    }
}
