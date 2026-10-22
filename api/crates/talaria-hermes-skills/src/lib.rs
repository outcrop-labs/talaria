// HERMES BUNDLED SKILLS — what we replace, what we keep, and the signposts
// that occupy the names agents reach for.
//
// Hermes ships a large library of bundled skills and syncs it into
// `/opt/data/skills` on every container start (stage2 runs
// `tools/skills_sync.py`). Some of those packs teach a PARALLEL WORKSPACE
// (Notion, Obsidian, Airtable, gh CLI, gws, Himalaya). Talaria is the system
// of record, so for each replaced pack a Talaria-authored skill of the same
// `name:` is seeded into `/opt/skills`, a root every agent lists under
// `skills.external_dirs`.
//
// THE SIGNPOST IS THE MECHANISM, not just a hint. `skills_sync` defers any
// bundled skill whose name a skill in `external_dirs` already provides: it
// never writes it into the local tree, and it removes a byte-identical stale
// copy an earlier sync left. So a replaced pack stays off an agent exactly
// as long as every one of its skill names has a signpost — which is what the
// tests below hold. Talaria used to `docker exec rm -rf` the replaced paths
// after every up and roll; that is gone, because Hermes now does the job
// itself from the first boot, with no window between start and prune.
//
// THE WEEKLY PROBLEM. Hermes adds packs. The classified catalog
// (`scripts/hermes-skill-authority.json`) is the tripwire: every snapshot
// entry — and every path the chassis smoke finds in the live image — is
// replaced, keepPrefix (apple/ only), or keepExact. `unclassified` is the
// public answer; an unclassified path fails the test and the smoke.
//
// SOURCE OF TRUTH is the JSON. Signpost directories live in
// scripts/skills/<signpost>/, same seed path as talaria-toolkit.

use serde::Deserialize;
use std::sync::LazyLock;

const AUTHORITY_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../scripts/hermes-skill-authority.json"
));

#[derive(Debug, Deserialize)]
struct Authority {
    replaced: Vec<Replaced>,
    #[serde(rename = "keepPrefixes")]
    keep_prefixes: Vec<String>,
    #[serde(rename = "keepExact")]
    keep_exact: Vec<String>,
    catalog: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Replaced {
    path: String,
    #[allow(dead_code)]
    names: Vec<String>,
    #[allow(dead_code)]
    signpost: String,
    #[allow(dead_code)]
    why: String,
}

fn authority() -> &'static Authority {
    static A: LazyLock<Authority> = LazyLock::new(|| {
        serde_json::from_str(AUTHORITY_JSON).expect("hermes-skill-authority.json parses")
    });
    &A
}

fn replaced_matches(path: &str, entry: &str) -> bool {
    entry == path || entry.starts_with(&format!("{path}/"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Replaced,
    Keep,
}

fn classify(entry: &str) -> Option<Class> {
    let a = authority();
    if a.replaced.iter().any(|r| replaced_matches(&r.path, entry)) {
        return Some(Class::Replaced);
    }
    if a.keep_exact.iter().any(|k| k == entry) {
        return Some(Class::Keep);
    }
    if a.keep_prefixes.iter().any(|p| entry.starts_with(p)) {
        return Some(Class::Keep);
    }
    None
}

/// The vendored snapshot of Hermes bundled pack paths.
pub fn catalog() -> &'static [String] {
    &authority().catalog
}

/// Pack paths (catalog snapshot or a live `find` of the image) that have no
/// replace / keepExact / keepPrefix classification. The chassis smoke feeds
/// this the image; the unit test feeds it the snapshot. Empty is the only
/// passing answer.
pub fn unclassified<'a>(entries: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    entries
        .into_iter()
        .filter(|e| classify(e).is_none())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;

    fn skills_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/skills")
    }

    #[test]
    fn the_authority_file_parses_and_the_catalog_is_nonempty() {
        assert!(!catalog().is_empty());
        let a = authority();
        assert!(!a.replaced.is_empty());
    }

    #[test]
    fn keep_prefixes_are_apple_only() {
        // Broader prefixes (creative/, web/) swallow next week's pack in that
        // family. apple/ is macOS-only and inert in these containers.
        assert_eq!(authority().keep_prefixes, ["apple/"]);
    }

    #[test]
    fn every_catalogued_hermes_pack_is_classified() {
        let snapshot: Vec<&str> = catalog().iter().map(|s| s.as_str()).collect();
        let unknown = unclassified(snapshot);
        assert!(
            unknown.is_empty(),
            "unclassified Hermes packs (add to replaced, keepExact, or keepPrefixes): {unknown:?}"
        );
    }

    #[test]
    fn replaced_paths_are_unique() {
        let paths: Vec<&str> = authority()
            .replaced
            .iter()
            .map(|r| r.path.as_str())
            .collect();
        let set: HashSet<&str> = paths.iter().copied().collect();
        assert_eq!(
            set.len(),
            paths.len(),
            "duplicate replaced paths: {paths:?}"
        );
    }

    /// Every `name:` a SKILL.md under scripts/skills declares in its
    /// frontmatter. Read from the frontmatter only, so a body that merely
    /// mentions a name ("never run `himalaya`") does not count as occupying it.
    fn signpost_names() -> HashSet<String> {
        let mut out = HashSet::new();
        for entry in fs::read_dir(skills_root()).unwrap() {
            let skill = entry.unwrap().path().join("SKILL.md");
            let Ok(body) = fs::read_to_string(&skill) else {
                continue;
            };
            let mut lines = body.lines();
            if lines.next().map(str::trim) != Some("---") {
                continue;
            }
            for line in lines {
                if line.trim() == "---" {
                    break;
                }
                if let Some(name) = line.strip_prefix("name:") {
                    out.insert(name.trim().to_string());
                }
            }
        }
        out
    }

    #[test]
    fn every_replaced_pack_has_a_signpost_skill_occupying_the_name() {
        let root = skills_root();
        let mut missing = Vec::new();
        for r in &authority().replaced {
            let skill = root.join(&r.signpost).join("SKILL.md");
            if !skill.is_file() {
                missing.push(format!(
                    "{} → scripts/skills/{}/SKILL.md is missing — {}",
                    r.path, r.signpost, r.why
                ));
            }
        }
        // EVERY name, not any one of them. Hermes defers a bundled skill only
        // when a skill in external_dirs carries ITS name, so one pack with
        // three skills needs three names occupied. Checking that any one
        // matched let `himalaya` and `email-inbox-triage` load on fresh
        // volumes while this test stayed green on the `email` signpost.
        let names = signpost_names();
        for r in &authority().replaced {
            for n in &r.names {
                if !names.contains(n) {
                    missing.push(format!(
                        "{} ships a skill named {n:?} and no scripts/skills/*/SKILL.md declares name: {n} — Hermes will sync it onto every agent",
                        r.path
                    ));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "replaced Hermes packs without a Talaria signpost:\n  {}",
            missing.join("\n  ")
        );
    }

    #[test]
    fn github_is_still_the_signpost_for_the_gh_cli_pack() {
        assert!(
            authority()
                .replaced
                .iter()
                .any(|r| r.path == "software-development/github" && r.signpost == "github")
        );
    }
}
