// One catalog entry's skill directory, read from GitHub at a pinned commit.
//
// THE PATH. Two API calls, then raw bytes — the shape Hermes' own GitHub
// source settled on (tools/skills_hub_github.py), for its reasons:
//   1. `GET /repos/{repo}/commits/HEAD` (as a bare sha) pins the revision,
//      so every byte below comes from the tree the paths were checked
//      against.
//   2. `GET /repos/{repo}/git/trees/{sha}?recursive=1` names every file and
//      its size before anything downloads. A repo too big for one tree
//      (`truncated`) is walked down to the skill directory instead.
//   3. `raw.githubusercontent.com/{repo}/{sha}/{path}` per file — outside
//      the API's rate limit (60 an hour, unauthenticated), so the limit
//      costs two calls per install however many files the skill carries.
// Never the repo tarball: the repos the catalog's skills live in run to
// 87 MB (hermes-agent) and 245 MB (K-Dense's science pack) for a skill of a
// few kilobytes.
//
// WHAT RIDES. Every regular file under the skill directory, minus hidden
// paths, VCS and build junk, symlinks (never followed), and anything that
// belongs to a deeper SKILL.md — a root skill does not drag its repo's
// other skills along. Caps per file, per skill and on depth refuse the
// whole fetch rather than installing a partial skill.
//
// OFFICIAL STUBS. Some of Hermes' official optional skills are catalog
// stubs whose frontmatter points upstream (`metadata.hermes.upstream:
// {repo, path}`); Hermes installs the upstream tree, and so does this. The
// result is judged as Hermes says it means to judge it — "trusted, not
// builtin, so a dangerous scan verdict still blocks" (skills_hub_official.py)
// — rather than as the official skill whose name it carries: an official
// source is never blocked at all, and the files are live third-party code.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde::Deserialize;
use talaria_safe_fetch::{SafeFetch, SafeResponse, safe_fetch};

use crate::index::CatalogEntry;
use crate::{normalize_skill_name, valid_repo};

const API: &str = "https://api.github.com/repos";
const RAW: &str = "https://raw.githubusercontent.com";
const USER_AGENT: &str = "talaria-skills-marketplace";
const API_TIMEOUT_MS: u64 = 30_000;
/// hermes-agent's recursive tree is ~6 MB (22k entries).
const TREE_MAX_BYTES: u64 = 24 * 1024 * 1024;
const RAW_TIMEOUT_MS: u64 = 30_000;
const RAW_CONCURRENCY: usize = 8;

const PER_FILE_MAX: u64 = 2 * 1024 * 1024;
const PER_SKILL_MAX_FILES: usize = 400;
const PER_SKILL_MAX_BYTES: u64 = 24 * 1024 * 1024;
/// Depth cap for a file inside the skill — skill layouts are shallow
/// (scripts/, references/, assets/); anything deeper is a repo, not a skill.
const MAX_PATH_DEPTH: usize = 8;

const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// The preview-then-install pair reads the same fetch; eight recent skills
/// is a browsing session, not a leak.
const CACHE_SLOTS: usize = 8;

/// A skill directory, fetched whole and ready to scan and install.
#[derive(Debug)]
pub struct FetchedSkill {
    /// The catalog identifier it was fetched for.
    pub id: String,
    /// Where the files actually came from (the upstream repo for a stub).
    pub repo: String,
    pub commit: String,
    pub dir: String,
    /// The directory name it installs under, in the skill alphabet.
    pub name: String,
    pub skill_md: String,
    /// Skill-relative paths and bytes, SKILL.md included, sorted by path.
    pub files: Vec<(String, Vec<u8>)>,
    /// What the scan is told the skill's source is — Hermes derives the
    /// trust level from it (tools/skills_guard.py `_resolve_trust_level`):
    /// "official" for an official skill, else the identifier.
    pub scan_source: String,
    /// The trust level the install policy judges with instead of the one
    /// Hermes resolves from `scan_source` — "trusted" for a followed
    /// official stub, `None` everywhere else.
    pub trust_override: Option<&'static str>,
    /// `owner/repo/path` when an official stub was followed upstream.
    pub upstream: Option<String>,
}

impl FetchedSkill {
    pub fn file_names(&self) -> Vec<&str> {
        self.files.iter().map(|(p, _)| p.as_str()).collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(default)]
    mode: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    sha: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Tree {
    #[serde(default)]
    tree: Vec<TreeEntry>,
    #[serde(default)]
    truncated: bool,
}

impl TreeEntry {
    fn is_regular_blob(&self) -> bool {
        self.kind == "blob" && self.mode != "120000"
    }
}

/// A GitHub answer the caller can act on, in words.
fn github_refusal(resp: &SafeResponse, repo: &str) -> String {
    let spent = resp
        .headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        == Some("0");
    match resp.status {
        403 | 429 if spent || resp.status == 429 => {
            "GitHub's API rate limit for this server is spent (60 calls an hour without a token) — try again within the hour".into()
        }
        404 => format!("{repo} is not on GitHub any more (or is private)"),
        409 => format!("{repo} is an empty repository"),
        s => format!("GitHub answered {s} for {repo}"),
    }
}

async fn github_get(url: &str, accept: &str, max_bytes: u64) -> Result<SafeResponse, String> {
    safe_fetch(
        url,
        SafeFetch {
            headers: vec![("Accept", accept), ("User-Agent", USER_AGENT)],
            timeout_ms: Some(API_TIMEOUT_MS),
            max_bytes: Some(max_bytes),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| format!("GitHub unreachable: {e}"))
}

async fn head_commit(repo: &str) -> Result<String, String> {
    let resp = github_get(
        &format!("{API}/{repo}/commits/HEAD"),
        "application/vnd.github.sha",
        4096,
    )
    .await?;
    if resp.status != 200 {
        return Err(github_refusal(&resp, repo));
    }
    let sha = String::from_utf8_lossy(&resp.body).trim().to_string();
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("GitHub returned no commit for {repo}"));
    }
    Ok(sha)
}

async fn tree(repo: &str, sha: &str, recursive: bool) -> Result<Tree, String> {
    let url = format!(
        "{API}/{repo}/git/trees/{sha}{}",
        if recursive { "?recursive=1" } else { "" }
    );
    let resp = github_get(&url, "application/vnd.github+json", TREE_MAX_BYTES).await?;
    if resp.status != 200 {
        return Err(github_refusal(&resp, repo));
    }
    serde_json::from_slice(&resp.body)
        .map_err(|e| format!("GitHub's tree for {repo} did not parse: {e}"))
}

/// The entries under `dir` (repo-relative paths), from one recursive tree —
/// or, when GitHub truncates that, by walking the tree down to `dir` and
/// reading only its subtree.
async fn entries_for(repo: &str, sha: &str, dir: Option<&str>) -> Result<Vec<TreeEntry>, String> {
    let whole = tree(repo, sha, true).await?;
    if !whole.truncated {
        return Ok(whole.tree);
    }
    let Some(dir) = dir.filter(|d| !d.is_empty()) else {
        return Err(format!(
            "{repo} is too large for GitHub to list in one tree, and the catalog does not say where in it the skill lives"
        ));
    };
    let mut at = sha.to_string();
    for seg in dir.split('/') {
        let level = tree(repo, &at, false).await?;
        let next = level
            .tree
            .iter()
            .find(|e| e.path == seg && e.kind == "tree")
            .ok_or_else(|| format!("{repo} has no {dir}/ at HEAD"))?;
        at = next.sha.clone();
    }
    let sub = tree(repo, &at, true).await?;
    if sub.truncated {
        return Err(format!("{repo}/{dir} is too large to be one skill"));
    }
    Ok(sub
        .tree
        .into_iter()
        .map(|mut e| {
            e.path = format!("{dir}/{}", e.path);
            e
        })
        .collect())
}

/// Where the skill lives: `path` itself when the catalog resolved it;
/// otherwise the one directory named `path` (a skills.sh slug) that holds a
/// SKILL.md, else a root SKILL.md — the two places Hermes looks.
fn locate(entries: &[TreeEntry], path: &str, exact: bool) -> Result<String, String> {
    if exact {
        return Ok(path.to_string());
    }
    let named: Vec<&str> = entries
        .iter()
        .filter(|e| e.is_regular_blob())
        .filter_map(|e| e.path.strip_suffix("/SKILL.md"))
        .filter(|parent| parent.rsplit('/').next() == Some(path))
        .collect();
    match named.as_slice() {
        [one] => Ok((*one).to_string()),
        [] if entries
            .iter()
            .any(|e| e.path == "SKILL.md" && e.is_regular_blob()) =>
        {
            Ok(String::new())
        }
        [] => Err(format!("no {path}/SKILL.md in the repo")),
        _ => Err(format!(
            "{} directories named {path} hold a SKILL.md",
            named.len()
        )),
    }
}

/// The files that ride with the skill at `dir`: (skill-relative path,
/// repo-relative path, size). Refuses on a cap; silently skips what never
/// rides (hidden, junk, symlinks, deeper skills).
fn members(entries: &[TreeEntry], dir: &str) -> Result<Vec<(String, String, u64)>, String> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let skill_md = format!("{prefix}SKILL.md");
    if !entries
        .iter()
        .any(|e| e.path == skill_md && e.is_regular_blob())
    {
        return Err(format!("{skill_md} is not in the repo at HEAD"));
    }
    // Deeper skill directories inside this one: their files are theirs.
    let deeper: HashSet<String> = entries
        .iter()
        .filter(|e| e.is_regular_blob())
        .filter_map(|e| e.path.strip_prefix(&prefix))
        .filter_map(|rel| rel.strip_suffix("/SKILL.md"))
        .map(|d| format!("{d}/"))
        .collect();
    let mut out = Vec::new();
    let mut total: u64 = 0;
    for e in entries {
        if !e.is_regular_blob() {
            continue;
        }
        let Some(rel) = e.path.strip_prefix(&prefix) else {
            continue;
        };
        let parts: Vec<&str> = rel.split('/').collect();
        if parts.len() > MAX_PATH_DEPTH
            || rel.ends_with(".pyc")
            || parts.iter().any(|p| {
                p.is_empty()
                    || p.starts_with('.')
                    || matches!(*p, "node_modules" | "__MACOSX" | "__pycache__")
            })
            || deeper.iter().any(|d| rel.starts_with(d.as_str()))
        {
            continue;
        }
        let size = e.size.unwrap_or(0);
        if size > PER_FILE_MAX {
            return Err(format!(
                "{rel} is {size} bytes — over the {PER_FILE_MAX}-byte per-file cap"
            ));
        }
        total += size;
        out.push((rel.to_string(), e.path.clone(), size));
    }
    if out.len() > PER_SKILL_MAX_FILES {
        return Err(format!(
            "the skill carries {} files — over the {PER_SKILL_MAX_FILES} cap",
            out.len()
        ));
    }
    if total > PER_SKILL_MAX_BYTES {
        return Err(format!(
            "the skill is {total} bytes — over the {PER_SKILL_MAX_BYTES}-byte cap"
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// One path segment, percent-encoded for a URL (RFC 3986 unreserved kept).
fn encode_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for b in seg.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

async fn raw_file(repo: &str, sha: &str, path: &str) -> Result<Vec<u8>, String> {
    let encoded: Vec<String> = path.split('/').map(encode_segment).collect();
    let url = format!("{RAW}/{repo}/{sha}/{}", encoded.join("/"));
    let resp = safe_fetch(
        &url,
        SafeFetch {
            timeout_ms: Some(RAW_TIMEOUT_MS),
            max_bytes: Some(PER_FILE_MAX),
            max_redirects: Some(0),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| format!("{path}: {e}"))?;
    if resp.status != 200 {
        return Err(format!(
            "raw.githubusercontent.com answered {} for {path}",
            resp.status
        ));
    }
    Ok(resp.body)
}

struct Dir {
    repo: String,
    commit: String,
    dir: String,
    files: Vec<(String, Vec<u8>)>,
}

/// Fetch one skill directory from `repo` at HEAD.
async fn fetch_dir(repo: &str, path: &str, exact: bool) -> Result<Dir, String> {
    if !valid_repo(repo) {
        return Err(format!("\"{repo}\" is not a GitHub owner/repo"));
    }
    let commit = head_commit(repo).await?;
    let entries = entries_for(repo, &commit, exact.then_some(path)).await?;
    let dir = locate(&entries, path, exact)?;
    let wanted = members(&entries, &dir)?;
    let files: Vec<(String, Vec<u8>)> = stream::iter(wanted)
        .map(|(rel, full, _)| {
            let (repo, commit) = (repo.to_string(), commit.clone());
            async move { raw_file(&repo, &commit, &full).await.map(|b| (rel, b)) }
        })
        .buffered(RAW_CONCURRENCY)
        .try_collect()
        .await?;
    Ok(Dir {
        repo: repo.to_string(),
        commit,
        dir,
        files,
    })
}

/// `metadata.hermes.upstream: {repo, path}` from a SKILL.md's frontmatter —
/// the pointer an official catalog stub carries. Validated the way Hermes
/// validates it: `repo` exactly `owner/name`, `path` a clean relative path.
pub fn upstream_pointer(skill_md: &str) -> Option<(String, String)> {
    let body = skill_md.strip_prefix('\u{feff}').unwrap_or(skill_md);
    let rest = body.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&rest[..end]).ok()?;
    let up = yaml.get("metadata")?.get("hermes")?.get("upstream")?;
    let repo = up
        .get("repo")?
        .as_str()?
        .trim()
        .trim_matches('/')
        .to_string();
    let path = up
        .get("path")?
        .as_str()?
        .trim()
        .trim_matches('/')
        .to_string();
    if !valid_repo(&repo)
        || path.is_empty()
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == ".." || s.contains('\\'))
    {
        return None;
    }
    Some((repo, path))
}

struct Cached {
    at: Instant,
    skill: Arc<FetchedSkill>,
}

static CACHE: LazyLock<Mutex<Vec<(String, Cached)>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Fetch a catalog entry's skill, cached for the preview → install pair.
pub async fn fetch_skill(entry: &CatalogEntry) -> Result<Arc<FetchedSkill>, String> {
    {
        let guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, c)) = guard
            .iter()
            .find(|(id, c)| *id == entry.id && c.at.elapsed() < CACHE_TTL)
        {
            return Ok(c.skill.clone());
        }
    }
    let first = fetch_dir(&entry.repo, &entry.path, entry.exact).await?;
    let skill_md_of = |files: &[(String, Vec<u8>)]| {
        files
            .iter()
            .find(|(p, _)| p == "SKILL.md")
            .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    };
    let first_md = skill_md_of(&first.files);
    // The install name is the catalog entry's own directory, stub or not.
    let raw_name = first
        .dir
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| entry.repo.rsplit('/').next().unwrap_or(&entry.repo))
        .to_string();
    let name = normalize_skill_name(&raw_name)
        .ok_or_else(|| format!("\"{raw_name}\" has no name in the skill alphabet"))?;
    let (dir, upstream, scan_source) = match (entry.source.as_str(), upstream_pointer(&first_md)) {
        ("official", Some((repo, path))) => {
            let up = fetch_dir(&repo, &path, true).await?;
            let label = format!("{repo}/{path}");
            (up, Some(label.clone()), label)
        }
        ("official", None) => (first, None, "official".to_string()),
        _ => (first, None, entry.id.clone()),
    };
    let skill_md = skill_md_of(&dir.files);
    if skill_md.trim().is_empty() {
        return Err("the skill's SKILL.md is empty".into());
    }
    let skill = Arc::new(FetchedSkill {
        id: entry.id.clone(),
        repo: dir.repo,
        commit: dir.commit,
        dir: dir.dir,
        name,
        skill_md,
        files: dir.files,
        trust_override: upstream.as_ref().map(|_| "trusted"),
        scan_source,
        upstream,
    });
    let mut guard = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    guard.retain(|(id, c)| c.at.elapsed() < CACHE_TTL && *id != entry.id);
    while guard.len() >= CACHE_SLOTS {
        guard.remove(0);
    }
    guard.push((
        entry.id.clone(),
        Cached {
            at: Instant::now(),
            skill: skill.clone(),
        },
    ));
    Ok(skill)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(path: &str, size: u64) -> TreeEntry {
        TreeEntry {
            path: path.into(),
            mode: "100644".into(),
            kind: "blob".into(),
            sha: String::new(),
            size: Some(size),
        }
    }

    fn link(path: &str) -> TreeEntry {
        TreeEntry {
            mode: "120000".into(),
            ..blob(path, 10)
        }
    }

    #[test]
    fn a_skill_takes_its_own_files_and_leaves_junk_links_and_deeper_skills() {
        let entries = vec![
            blob("README.md", 1),
            blob("skills/a/SKILL.md", 10),
            blob("skills/a/references/notes.md", 5),
            blob("skills/a/.hidden/x.md", 5),
            blob("skills/a/node_modules/x.js", 5),
            blob("skills/a/scripts/__pycache__/x.pyc", 5),
            blob("skills/a/scripts/run.py", 5),
            link("skills/a/escape.md"),
            blob("skills/a/inner/SKILL.md", 5),
            blob("skills/a/inner/own.md", 5),
            blob("skills/b/SKILL.md", 5),
        ];
        let got: Vec<String> = members(&entries, "skills/a")
            .unwrap()
            .into_iter()
            .map(|(rel, _, _)| rel)
            .collect();
        assert_eq!(
            got,
            vec!["SKILL.md", "references/notes.md", "scripts/run.py"]
        );
    }

    #[test]
    fn a_root_skill_does_not_drag_the_repos_other_skills_along() {
        let entries = vec![
            blob("SKILL.md", 10),
            blob("scripts/tangle.ts", 5),
            blob("skills/other/SKILL.md", 5),
            blob("skills/other/x.md", 5),
        ];
        let got: Vec<String> = members(&entries, "")
            .unwrap()
            .into_iter()
            .map(|(rel, _, _)| rel)
            .collect();
        assert_eq!(got, vec!["SKILL.md", "scripts/tangle.ts"]);
    }

    #[test]
    fn caps_refuse_the_whole_skill_and_a_missing_skill_md_is_an_error() {
        let big = vec![blob("s/SKILL.md", 1), blob("s/huge.bin", PER_FILE_MAX + 1)];
        assert!(members(&big, "s").unwrap_err().contains("per-file cap"));
        let many: Vec<TreeEntry> = std::iter::once(blob("s/SKILL.md", 1))
            .chain((0..PER_SKILL_MAX_FILES).map(|i| blob(&format!("s/f{i}.md"), 1)))
            .collect();
        assert!(members(&many, "s").unwrap_err().contains("files"));
        let linked = vec![link("s/SKILL.md")];
        assert!(
            members(&linked, "s")
                .unwrap_err()
                .contains("not in the repo")
        );
        assert!(members(&[blob("t/SKILL.md", 1)], "s").is_err());
    }

    #[test]
    fn an_unresolved_slug_is_found_by_name_or_at_the_root() {
        let entries = vec![
            blob("bundles/workflow/skills/1-3-1/SKILL.md", 1),
            blob("bundles/other/SKILL.md", 1),
        ];
        assert_eq!(
            locate(&entries, "1-3-1", false).unwrap(),
            "bundles/workflow/skills/1-3-1"
        );
        assert_eq!(locate(&entries, "skills/x", true).unwrap(), "skills/x");
        let rooted = vec![blob("SKILL.md", 1), blob("README.md", 1)];
        assert_eq!(locate(&rooted, "anything", false).unwrap(), "");
        let twice = vec![blob("a/dup/SKILL.md", 1), blob("b/dup/SKILL.md", 1)];
        assert!(
            locate(&twice, "dup", false)
                .unwrap_err()
                .contains("2 directories")
        );
        assert!(locate(&[blob("x/SKILL.md", 1)], "nope", false).is_err());
    }

    #[test]
    fn an_official_stub_points_upstream_and_a_bad_pointer_is_ignored() {
        let stub = "---\nname: brag\nmetadata:\n  hermes:\n    upstream:\n      repo: latent-spaces/brag\n      path: skills/brag\n---\n# Brag\n";
        assert_eq!(
            upstream_pointer(stub),
            Some(("latent-spaces/brag".into(), "skills/brag".into()))
        );
        let escape = stub.replace("skills/brag", "../etc");
        assert_eq!(upstream_pointer(&escape), None);
        let not_repo = stub.replace("latent-spaces/brag", "just-a-name");
        assert_eq!(upstream_pointer(&not_repo), None);
        assert_eq!(upstream_pointer("---\nname: plain\n---\n# Plain\n"), None);
        assert_eq!(upstream_pointer("# no frontmatter"), None);
    }

    #[test]
    fn path_segments_encode_for_the_raw_url() {
        assert_eq!(encode_segment("notes v2.md"), "notes%20v2.md");
        assert_eq!(encode_segment("ünï"), "%C3%BCn%C3%AF");
        assert_eq!(encode_segment("a-b_c.d~e"), "a-b_c.d~e");
    }
}
