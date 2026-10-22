// The skills marketplace — Hermes' own skills catalog, browsed from the
// Studio, scanned the way Hermes scans, installed into Talaria's skill roots.
// Four moves, one module each:
//
//   index    the catalog: Hermes' official skills index (the JSON its own
//            `hermes skills search` reads, rebuilt daily on its docs site)
//            mirrored into `skills_index_entries` by a scheduler job, and
//            searched there — the picker never waits on the network
//   fetch    one catalog entry's skill directory, read from GitHub at a
//            pinned commit: the git tree names the files, raw.github serves
//            them. Per skill, never the whole repo — the repos that hold the
//            catalog's skills run to hundreds of megabytes
//   guard    the pre-install scan: Hermes' own Skills Guard
//            (tools/skills_guard.py) run in a one-shot, network-less,
//            read-only container from the fleet's own Hermes image, with
//            Hermes' own install policy deciding — same rules, same version
//            as the agents that will load the skill
//   (install) lives in the route: it writes the fetched files into the
//            owner's skill root through talaria-agent-skills'
//            install_skill_dir, which owns the never-clobber and path-safety
//            invariants
//
// Trust model: a skill is markdown plus support files the agent READS (and
// may run, for scripted skills) — install is gated on canEdit for the
// destination, the scan blocks what Hermes would block (any high or critical
// finding on a community skill, a critical one on a trusted skill), and the
// UI says what was found. Names normalize into the skill alphabet
// ([a-z0-9._-], first char a letter or digit).

pub mod fetch;
pub mod guard;
pub mod index;

pub use fetch::{FetchedSkill, fetch_skill};
pub use guard::{Finding, ScanReport, scan};
pub use index::{
    CatalogEntry, entry, register_skills_index_refresh_job, search, skills_index_refresh_job_spec,
};

/// `owner/repo` with each part in GitHub's alphabet. One slash, no
/// traversal — `..` is all-dots and thus alphabet-legal, so it is refused
/// by name — and every GitHub URL is format!-built from the validated
/// parts only.
pub fn valid_repo(repo: &str) -> bool {
    let Some((org, name)) = repo.split_once('/') else {
        return false;
    };
    let part = |p: &str| {
        !p.is_empty()
            && p.len() <= 100
            && p != "."
            && p != ".."
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    part(org) && part(name)
}

/// Coerce a directory name into the skill alphabet (the routes' NAME
/// regex: `/^[a-z0-9][a-z0-9._-]*$/`). None when nothing alphabet-shaped
/// survives — the caller refuses the skill and says so.
pub fn normalize_skill_name(raw: &str) -> Option<String> {
    let mapped: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    // Collapse the replacement's runs, then trim edges — the alphabet bars
    // a leading dash.
    let mut out = mapped;
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let trimmed = out.trim_matches('-').to_string();
    let b = trimmed.as_bytes();
    if b.is_empty() || !(b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) {
        return None;
    }
    Some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_validation_accepts_github_shapes_and_refuses_the_rest() {
        assert!(valid_repo("obra/superpowers"));
        assert!(valid_repo("a.b-c_d/e.f-g_h"));
        assert!(!valid_repo("obra"));
        assert!(!valid_repo("obra/"));
        assert!(!valid_repo("/x"));
        assert!(!valid_repo("a/b/c"));
        assert!(!valid_repo("../etc"));
        assert!(!valid_repo("org/repo\u{200b}"));
        assert!(!valid_repo("org/spa ce"));
    }

    #[test]
    fn names_normalize_into_the_alphabet_or_vanish() {
        assert_eq!(
            normalize_skill_name("Draw.io-Skill").as_deref(),
            Some("draw.io-skill")
        );
        assert_eq!(normalize_skill_name("A B").as_deref(), Some("a-b"));
        assert_eq!(normalize_skill_name("-edge-").as_deref(), Some("edge"));
        assert_eq!(normalize_skill_name("Über!").as_deref(), Some("ber"));
        assert_eq!(
            normalize_skill_name("My--Fancy Skill").as_deref(),
            Some("my-fancy-skill")
        );
        assert_eq!(normalize_skill_name("!!!"), None);
        assert_eq!(normalize_skill_name(""), None);
    }
}
