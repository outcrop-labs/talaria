// The skills marketplace — Hermes Atlas's ranked skill catalog, browsed from
// the Studio, installed from GitHub. Three moves:
//
//   entries()   the /lists/top-skills page, fetched live and parsed (the site
//               is server-rendered HTML; there is no JSON API), cached 15min
//               and filtered by the picker's query
//   scan()      one repo's codeload tarball (HEAD = default branch), scanned
//               for SKILL.md directories — the agentskills.io unit — and kept
//               whole in a small bounded cache so the discover→install pair
//               downloads once
//   (install)   lives in the route: it walks scan()'s files into the owner's
//               skill root through talaria-agent-skills' install_skill_dir,
//               which owns the never-clobber and path-safety invariants
//
// Trust model: a skill is markdown plus support files the agent READS (and
// may run, for scripted skills) — the same trust as pasting a SKILL.md by
// hand in the editor, which is why install is gated on canEdit and the UI
// says so. Names normalize into the skill alphabet ([a-z0-9._-], first char
// a letter or digit): a repo's root SKILL.md takes the repo's own name.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use talaria_safe_fetch::{SafeFetch, safe_fetch};

const LIST_URL: &str = "https://hermesatlas.com/lists/top-skills";
const LIST_CACHE: Duration = Duration::from_secs(15 * 60);
const LIST_TIMEOUT_MS: u64 = 10_000;
const LIST_MAX_BYTES: u64 = 2 * 1024 * 1024;

const TARBALL_TIMEOUT_MS: u64 = 60_000;
/// The whole repo, every skill's support files included. Real skill packs
/// measure in kilobytes (superpowers: 640KB gzipped); anything past this is
/// a repo that is not a skill pack.
const TARBALL_MAX_BYTES: u64 = 32 * 1024 * 1024;
const TARBALL_CACHE: Duration = Duration::from_secs(10 * 60);
/// Cache slots. Tarballs are held WHOLE in memory for the install half of
/// the pair; eight recent repos is a browsing session, not a leak.
const TARBALL_SLOTS: usize = 8;

const PER_FILE_MAX: usize = 2 * 1024 * 1024;
const PER_SKILL_MAX_FILES: usize = 400;
const PER_SKILL_MAX_BYTES: usize = 24 * 1024 * 1024;
/// Unpacked total across the repo — the gz cap alone lets a bomb through.
const REPO_TOTAL_MAX: usize = 96 * 1024 * 1024;
/// Depth cap for a file inside the tarball — skill layouts are shallow
/// (scripts/, references/, assets/); anything deeper is a repo, not a skill.
const MAX_PATH_DEPTH: usize = 8;

/// One ranked repo on the list. `repo` ("obra/superpowers") is the install
/// identity — GitHub owner/name — and the only field the wire contract
/// needs; everything else is shelf dressing.
#[derive(Debug, Clone, Serialize)]
pub struct MarketplaceEntry {
    pub repo: String,
    pub org: String,
    pub name: String,
    pub description: String,
    /// The site's own display form ("290.0K"), not a number — stars here are
    /// marketing, and re-parsing their label would only invent precision.
    pub stars: String,
    pub rank: usize,
}

/// One SKILL.md directory found inside a repo tarball.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredSkill {
    /// Normalized into the skill alphabet — the directory name it will
    /// install under.
    pub name: String,
    /// The summarizer fallback line (frontmatter description or first
    /// prose) from the SKILL.md itself.
    pub summary: String,
    /// Files beside the SKILL.md as relative paths ("references/notes.md"),
    /// sorted — the shape the install writes and the picker previews.
    pub files: Vec<String>,
    /// SKILL.md body, carried so the route can hand the install one read
    /// of the scan instead of re-walking the tarball.
    #[serde(skip)]
    pub skill_md: String,
    /// The files' bytes keyed by the same relative paths as `files`
    /// (SKILL.md included).
    #[serde(skip)]
    pub blobs: HashMap<String, Vec<u8>>,
}

/// A repo scanned for skills — the cached unit.
#[derive(Debug, Default)]
pub struct RepoScan {
    pub repo: String,
    pub skills: Vec<DiscoveredSkill>,
}

impl RepoScan {
    pub fn skill(&self, name: &str) -> Option<&DiscoveredSkill> {
        self.skills.iter().find(|s| s.name == name)
    }
}

/// `owner/repo` with each part in GitHub's alphabet. One slash, no
/// traversal — `..` is all-dots and thus alphabet-legal, so it is refused
/// by name — and the codeload URL is format!-built from the validated
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

/// Coerce a tar directory name into the skill alphabet (the routes' NAME
/// regex: `/^[a-z0-9][a-z0-9._-]*$/`). None when nothing alphabet-shaped
/// survives — the caller skips the skill and says so.
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

// ── The list half ────────────────────────────────────────────────────────────

/// Minimal entity unescape — the list's descriptions carry `&amp;` and the
/// odd `&#39;`; a full HTML unescaper would be a dependency for two rows.
fn html_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Drop any tag the description might carry. The observed markup has none;
/// this stands so a future `<em>` cannot leak angle brackets into the wire.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn text_between<'a>(hay: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let i = hay.find(open)? + open.len();
    let j = hay[i..].find(close)? + i;
    Some(&hay[i..j])
}

/// Parse the list page. The repeating unit (2026-09 snapshot):
/// `<a class="list-row" href="/projects/org/repo"> … <div
/// class="list-cell-desc">…</div> … <div class="list-cell-stars">★ 1</div>
/// </a>` — anchor-scoped, so a desc or stars cell missing from one row
/// degrades that row (empty description, "—") rather than the whole page.
/// Rank is document order; the page is sorted by stars already.
pub fn parse_list_html(html: &str) -> Vec<MarketplaceEntry> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find(r#"<a class="list-row""#) {
        let after = &rest[i..];
        let Some(len) = after.find("</a>") else {
            break;
        };
        let row = &after[..len];
        rest = &after[len..];
        let Some(href) = text_between(row, r#"href="/projects/"#, "\"") else {
            continue;
        };
        let Some((org, name)) = href.split_once('/') else {
            continue;
        };
        if org.is_empty() || name.is_empty() {
            continue;
        }
        let desc = text_between(row, r#"<div class="list-cell-desc">"#, "</div>")
            .map(|d| strip_tags(d).trim().to_string())
            .unwrap_or_default();
        let stars = text_between(row, "<div class=\"list-cell-stars\">★", "</div>")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "—".into());
        out.push(MarketplaceEntry {
            repo: format!("{org}/{name}"),
            org: html_unescape(org),
            name: html_unescape(name),
            description: html_unescape(&desc),
            stars,
            rank: out.len() + 1,
        });
    }
    out
}

/// Case-insensitive substring across repo and description — the picker
/// ships the raw query and renders what comes back.
fn matches(entry: &MarketplaceEntry, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty()
        || entry.repo.to_lowercase().contains(&q)
        || entry.description.to_lowercase().contains(&q)
}

// ── The tarball half ─────────────────────────────────────────────────────────

/// Path components of a tar entry, minus GitHub's `{repo}-{ref}/` root.
/// None for anything the scan refuses: absolute paths and windows drives,
/// ANY `..` (checked on the raw path — a leading `..` must not get eaten as
/// the root dir), VCS/junk dirs, hidden files, or too deep.
fn clean_components(path: &str) -> Option<Vec<String>> {
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return None;
    }
    if path.split('/').any(|p| p == "..") {
        return None;
    }
    let mut parts: Vec<String> = path.split('/').map(str::to_string).collect();
    // codeload always wraps in one leading dir; a flat archive without it
    // is still readable, so only strip when present.
    if parts.len() > 1 {
        parts.remove(0);
    }
    if parts.is_empty() || parts.len() > MAX_PATH_DEPTH {
        return None;
    }
    for p in &parts {
        if p.is_empty() || p == ".." || p.starts_with('.') || p == "node_modules" || p == "__MACOSX"
        {
            return None;
        }
    }
    Some(parts)
}

/// Scan a tar.gz byte stream into skills. Every dir holding a SKILL.md
/// becomes one skill (a root-level SKILL.md is the whole repo as one skill,
/// named for the repo); each file belongs to the DEEPEST skill dir
/// enclosing it, so nested packs stay separate and a support tree
/// (`references/…`, `scripts/…`) rides with its skill. Two dirs that
/// normalize to the same name collide — first-in-archive-order wins,
/// deterministic for a given tarball.
pub fn scan_tarball(bytes: &[u8], repo: &str) -> Result<RepoScan, String> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    // (parts, bytes) — attribution and the skill-relative paths happen
    // once the skill dirs are known.
    let mut files: Vec<(Vec<String>, Vec<u8>)> = Vec::new();
    let mut total: usize = 0;
    for entry in archive
        .entries()
        .map_err(|e| format!("unreadable tarball: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("unreadable tarball: {e}"))?;
        if !entry.header().entry_type().is_file() {
            continue; // dirs are implicit in the writes; symlinks never ride
        }
        let path = entry
            .path()
            .map_err(|e| format!("unreadable path: {e}"))?
            .to_string_lossy()
            .into_owned();
        let Some(parts) = clean_components(&path) else {
            continue;
        };
        let mut blob = Vec::new();
        entry
            .read_to_end(&mut blob)
            .map_err(|e| format!("unreadable file {path}: {e}"))?;
        if blob.len() > PER_FILE_MAX {
            return Err(format!(
                "{path} is {} bytes — over the {} per-file cap",
                blob.len(),
                PER_FILE_MAX
            ));
        }
        total += blob.len();
        if total > REPO_TOTAL_MAX {
            return Err(format!("repo unpacks past the {REPO_TOTAL_MAX}-byte cap"));
        }
        files.push((parts, blob));
    }

    // The skill dirs, in first-seen order: every SKILL.md's parent. A dir
    // whose name vanishes under normalization is skipped whole — it never
    // becomes a skill dir, so nothing attributes to it.
    let repo_name = repo.rsplit('/').next().unwrap_or(repo);
    let mut skill_dirs: Vec<(Vec<String>, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (parts, _) in &files {
        if parts.last().is_some_and(|n| n == "SKILL.md") {
            let dir = parts[..parts.len() - 1].to_vec();
            let raw = dir.last().map(String::as_str).unwrap_or(repo_name);
            if let Some(name) = normalize_skill_name(raw)
                && seen.insert(name.clone())
            {
                skill_dirs.push((dir, name));
            }
        }
    }

    let mut skills: Vec<DiscoveredSkill> = Vec::new();
    for (dir, name) in &skill_dirs {
        // A file belongs here when this is the DEEPEST skill dir enclosing
        // it — nested packs stay theirs, support trees ride with their
        // skill — and lands under its skill-relative path.
        let deeper = |p: &[String]| {
            skill_dirs
                .iter()
                .any(|(d, _)| d.len() > dir.len() && p.starts_with(d))
        };
        let mut blobs = HashMap::new();
        let mut size = 0;
        for (parts, blob) in &files {
            if parts.starts_with(dir) && !deeper(parts) {
                blobs.insert(parts[dir.len()..].join("/"), blob.clone());
                size += blob.len();
            }
        }
        if blobs.is_empty() {
            continue; // lost every file to a name collision — nothing to install
        }
        if blobs.len() > PER_SKILL_MAX_FILES {
            return Err(format!(
                "skill {name} carries {} files — over the {PER_SKILL_MAX_FILES} cap",
                blobs.len()
            ));
        }
        if size > PER_SKILL_MAX_BYTES {
            return Err(format!(
                "skill {name} unpacks to {size} bytes — over the cap"
            ));
        }
        let skill_md = blobs
            .get("SKILL.md")
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        let mut names: Vec<String> = blobs.keys().cloned().collect();
        names.sort();
        skills.push(DiscoveredSkill {
            name: name.clone(),
            summary: talaria_agent_skills::summarize_fallback(&skill_md),
            files: names,
            skill_md,
            blobs,
        });
    }
    if skills.is_empty() {
        return Err(format!("{repo} carries no SKILL.md directories"));
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(RepoScan {
        repo: repo.to_string(),
        skills,
    })
}

// ── The singleton: caches over the two fetches ───────────────────────────────

struct Cache<T> {
    at: std::time::Instant,
    value: Arc<T>,
}

pub struct Marketplace {
    list: Mutex<Option<Cache<Vec<MarketplaceEntry>>>>,
    tarballs: Mutex<Vec<(String, Cache<RepoScan>)>>,
}

impl Marketplace {
    fn new() -> Self {
        Self {
            list: Mutex::new(None),
            tarballs: Mutex::new(Vec::new()),
        }
    }

    /// The ranked list, filtered. Cache-first: a cold fetch failure is an
    /// error the picker surfaces ("didn't load"), a warm one serves stale.
    pub async fn entries(&self, q: &str) -> Result<Vec<MarketplaceEntry>, String> {
        let cached = {
            let guard = self.list.lock().unwrap_or_else(|p| p.into_inner());
            guard
                .as_ref()
                .filter(|c| c.at.elapsed() < LIST_CACHE)
                .map(|c| c.value.clone())
        };
        let list = match cached {
            Some(l) => l,
            None => {
                let resp = safe_fetch(
                    LIST_URL,
                    SafeFetch {
                        timeout_ms: Some(LIST_TIMEOUT_MS),
                        max_bytes: Some(LIST_MAX_BYTES),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| format!("Hermes Atlas unreachable: {e}"))?;
                if !(200..300).contains(&resp.status) {
                    return Err(format!(
                        "Hermes Atlas answered {} for the skills list",
                        resp.status
                    ));
                }
                let html = String::from_utf8_lossy(&resp.body).into_owned();
                let parsed = parse_list_html(&html);
                if parsed.is_empty() {
                    return Err("Hermes Atlas's skills list parsed to zero rows".into());
                }
                let arc = Arc::new(parsed);
                *self.list.lock().unwrap_or_else(|p| p.into_inner()) = Some(Cache {
                    at: std::time::Instant::now(),
                    value: arc.clone(),
                });
                arc
            }
        };
        let q = q.trim();
        Ok(if q.is_empty() {
            list.as_ref().clone()
        } else {
            list.iter().filter(|e| matches(e, q)).cloned().collect()
        })
    }

    /// One repo's skills, cached. The install half re-reads the same scan,
    /// so a click-through pair downloads the tarball once.
    pub async fn scan(&self, repo: &str) -> Result<Arc<RepoScan>, String> {
        if !valid_repo(repo) {
            return Err(format!("\"{repo}\" is not a GitHub owner/repo"));
        }
        let cached = {
            let guard = self.tarballs.lock().unwrap_or_else(|p| p.into_inner());
            guard
                .iter()
                .find(|(r, c)| r == repo && c.at.elapsed() < TARBALL_CACHE)
                .map(|(_, c)| c.value.clone())
        };
        if let Some(scan) = cached {
            return Ok(scan);
        }
        let url = format!("https://codeload.github.com/{repo}/tar.gz/HEAD");
        let resp = safe_fetch(
            &url,
            SafeFetch {
                timeout_ms: Some(TARBALL_TIMEOUT_MS),
                max_bytes: Some(TARBALL_MAX_BYTES),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("{repo} unreachable: {e}"))?;
        if !(200..300).contains(&resp.status) {
            return Err(format!("GitHub answered {} for {repo}", resp.status));
        }
        let scan = Arc::new(scan_tarball(&resp.body, repo)?);
        let mut guard = self.tarballs.lock().unwrap_or_else(|p| p.into_inner());
        // Evict expired first, then oldest, so the cache is a recent-set
        // rather than a leak.
        guard.retain(|(_, c)| c.at.elapsed() < TARBALL_CACHE);
        while guard.len() >= TARBALL_SLOTS {
            guard.remove(0);
        }
        guard.push((
            repo.to_string(),
            Cache {
                at: std::time::Instant::now(),
                value: scan.clone(),
            },
        ));
        Ok(scan)
    }
}

/// The production singleton. No scheduler warm: one list fetch per quarter
/// hour is not worth a job (the MCP shelf fans out per publisher; this is
/// one page).
pub fn marketplace() -> &'static Marketplace {
    static MP: LazyLock<Marketplace> = LazyLock::new(Marketplace::new);
    &MP
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn row(org: &str, name: &str, desc: &str, stars: &str) -> String {
        format!(
            r#"<a class="list-row" href="/projects/{org}/{name}"><div class="list-rank">1</div><div class="list-cell-body"><div class="list-cell-name"><span class="org">{org} /</span> {name}</div><div class="list-cell-desc">{desc}</div></div><div class="list-cell-stars">★ {stars}</div></a>"#
        )
    }

    #[test]
    fn the_list_parses_rank_desc_and_stars() {
        let html = format!(
            "<main>{}{}</main>",
            row("obra", "superpowers", "TDD &amp; DRI frameworks", "290.0K"),
            row(
                "tlehman",
                "litprog-skill",
                "Literate programming &#39;skill&#39;",
                "270"
            )
        );
        let es = parse_list_html(&html);
        assert_eq!(es.len(), 2);
        assert_eq!(es[0].repo, "obra/superpowers");
        assert_eq!(es[0].rank, 1);
        assert_eq!(es[0].stars, "290.0K");
        assert_eq!(es[0].description, "TDD & DRI frameworks");
        assert_eq!(es[1].rank, 2);
        assert_eq!(es[1].description, "Literate programming 'skill'");
    }

    #[test]
    fn a_row_without_a_desc_degrades_alone() {
        let html = r#"<a class="list-row" href="/projects/a/b"><div class="list-cell-name"><span class="org">a /</span> b</div></a>"#;
        let es = parse_list_html(html);
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].description, "");
        assert_eq!(es[0].stars, "—");
    }

    #[test]
    fn search_spans_repo_and_description_case_blind() {
        let es = parse_list_html(&row("obra", "superpowers", "agentic skills framework", "1"));
        assert_eq!(es.iter().filter(|e| matches(e, "OBRA")).count(), 1);
        assert_eq!(es.iter().filter(|e| matches(e, "framework")).count(), 1);
        assert_eq!(es.iter().filter(|e| matches(e, "notion")).count(), 0);
    }

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

    fn build_tar(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body) in files {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, body.as_bytes())
                .unwrap();
        }
        let raw = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&raw).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn hostile_paths_die_at_the_door() {
        // The tar BUILDER refuses `..` members, so the traversal shapes are
        // unit-tested at the cleaner, which is where they die in production.
        assert!(clean_components("../escape.md").is_none());
        assert!(clean_components("/abs.md").is_none());
        assert!(clean_components("C:\\windows\\x").is_none());
        assert!(clean_components("r-HEAD/.git/config").is_none());
        assert!(clean_components("r-HEAD/a/../../b.md").is_none());
        assert_eq!(
            clean_components("r-HEAD/skills/a/SKILL.md").unwrap(),
            vec!["skills", "a", "SKILL.md"]
        );
    }

    #[test]
    fn a_root_skill_md_makes_the_repo_one_skill() {
        let tgz = build_tar(&[
            (
                "litprog-HEAD/SKILL.md",
                "---\nname: litprog\ndescription: Literate programming\n---\n# Litprog\n",
            ),
            ("litprog-HEAD/README.md", "# readme"),
            ("litprog-HEAD/scripts/tangle.ts", "export {}"),
        ]);
        let scan = scan_tarball(&tgz, "tlehman/litprog-skill").unwrap();
        assert_eq!(scan.skills.len(), 1);
        let s = &scan.skills[0];
        assert_eq!(s.name, "litprog-skill");
        assert_eq!(s.summary, "Literate programming");
        assert_eq!(s.files, vec!["README.md", "SKILL.md", "scripts/tangle.ts"]);
        assert!(s.blobs.contains_key("scripts/tangle.ts"));
    }

    #[test]
    fn a_pack_scans_each_skill_dir_and_skips_junk() {
        let tgz = build_tar(&[
            ("p-HEAD/.git/config", "junk"),
            (
                "p-HEAD/skills/brainstorming/SKILL.md",
                "# Brainstorming\n\nWays to find ideas.",
            ),
            ("p-HEAD/skills/brainstorming/references/notes.md", "x"),
            (
                "p-HEAD/skills/Executing_Plans/SKILL.md",
                "# Executing plans",
            ),
            ("p-HEAD/node_modules/x/y.js", "junk"),
            ("p-HEAD/README.md", "# not a skill"),
        ]);
        let scan = scan_tarball(&tgz, "obra/p").unwrap();
        let names: Vec<&str> = scan.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["brainstorming", "executing_plans"]);
        let brain = scan.skill("brainstorming").unwrap();
        assert_eq!(brain.files, vec!["SKILL.md", "references/notes.md"]);
        assert_eq!(
            brain.summary, "Ways to find ideas.",
            "frontmatter absent → first prose line"
        );
    }

    #[test]
    fn a_nested_pack_stays_separate_from_its_parent_dir() {
        let tgz = build_tar(&[
            ("p-HEAD/skills/outer/SKILL.md", "# Outer"),
            ("p-HEAD/skills/outer/notes.md", "outer note"),
            ("p-HEAD/skills/outer/inner/SKILL.md", "# Inner"),
            ("p-HEAD/skills/outer/inner/notes.md", "inner note"),
        ]);
        let scan = scan_tarball(&tgz, "a/p").unwrap();
        let outer = scan.skill("outer").unwrap();
        let inner = scan.skill("inner").unwrap();
        assert_eq!(outer.files, vec!["SKILL.md", "notes.md"]);
        assert_eq!(inner.files, vec!["SKILL.md", "notes.md"]);
        assert_eq!(
            outer
                .blobs
                .get("notes.md")
                .map(|b| String::from_utf8_lossy(b).into_owned()),
            Some("outer note".into())
        );
    }

    #[test]
    fn a_repo_without_skills_is_an_error_not_an_empty_shelf() {
        let tgz = build_tar(&[("r-HEAD/README.md", "# readme")]);
        assert!(
            scan_tarball(&tgz, "a/r")
                .unwrap_err()
                .contains("no SKILL.md")
        );
    }

    #[test]
    fn an_oversized_file_refuses_the_whole_scan() {
        let big = "x".repeat(PER_FILE_MAX + 1);
        let tgz = build_tar(&[("r-HEAD/skills/a/SKILL.md", big.as_str())]);
        assert!(
            scan_tarball(&tgz, "a/r")
                .unwrap_err()
                .contains("per-file cap")
        );
    }
}
