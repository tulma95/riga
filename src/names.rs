//! Candidate values produced by the built-in sources, and the one builder every source uses.
use std::collections::BTreeMap;
const MAX_NAME_CHARS: usize = 256;
const MAX_DESCRIPTION_CHARS: usize = 100;
#[derive(Debug, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub description: String,
}
/// Collects names read from untrusted files: names with control or bidirectional characters, or
/// empty or overlong ones, are dropped; the first description of a duplicate wins; a description
/// with such characters is blanked and the rest is cut to 100 characters. `finish` sorts by name and
/// only then applies the cap, so an oversized source yields the first names alphabetically rather
/// than whatever the file system listed first.
pub struct Names {
    cap: usize,
    items: BTreeMap<String, String>,
}
impl Names {
    pub fn new(cap: usize) -> Self {
        Names {
            cap,
            items: BTreeMap::new(),
        }
    }
    pub fn push(&mut self, name: &str, description: &str) {
        if name.is_empty()
            || name.chars().count() > MAX_NAME_CHARS
            || name.chars().any(crate::spec::deceptive)
            || self.items.contains_key(name)
        {
            return;
        }
        let description = description.trim();
        let description = if description.chars().any(crate::spec::deceptive) {
            String::new()
        } else {
            description.chars().take(MAX_DESCRIPTION_CHARS).collect()
        };
        self.items.insert(name.into(), description);
    }
    pub fn finish(self) -> Vec<Ref> {
        self.items
            .into_iter()
            .take(self.cap)
            .map(|(name, description)| Ref { name, description })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_dedups_and_sorts_before_capping() {
        let mut names = Names::new(3);
        for n in ["zeta", "bad\u{202e}", "", "beta", "zeta", "alpha", "gamma"] {
            names.push(n, "first");
        }
        names.push("beta", "second");
        names.push("delta", "bad\u{1b}");
        names.push("x", &"é".repeat(150));
        let out = names.finish();
        let got: Vec<_> = out.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(got, ["alpha", "beta", "delta"]);
        assert_eq!(out[1].description, "first");
        assert_eq!(out[2].description, "");
        let mut long = Names::new(9);
        long.push("x", &"é".repeat(150));
        assert_eq!(long.finish()[0].description.chars().count(), 100);
    }
}
