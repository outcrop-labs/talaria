// THE CHECK — what Hermes' `stable` tag names right now.
//
// One anonymous manifest HEAD for the digest, and (for a digest not seen
// before) a tag list and a HEAD per recent `vX.Y.Z` tag to find which release
// shares it. On Docker Hub none of these is a pull: the anonymous limit
// counts manifest GETs only, and a HEAD leaves `ratelimit-remaining` where it
// was (measured 2026-11-03, four HEADs in a row at 98 of 100). Nothing here
// GETs a manifest or a blob.
//
// Everything a registry answers is untrusted: the digest must be
// `sha256:<64 hex>` (the registry crate refuses anything else), a tag is
// kept only as `vX.Y.Z`, and the record is written from those alone.

use sqlx::PgPool;

use talaria_hermes_images as images;
use talaria_update_registry::{ImageRef, PublicRepo, parse_image_ref};

/// The tag the channel follows, and the only one.
pub const STABLE_REF_DEFAULT: &str = "nousresearch/hermes-agent:stable";

/// How many of the newest release tags a new digest is matched against.
const VERSION_HEADS: usize = 8;

/// The reference the channel follows: `TALARIA_HERMES_STABLE_REF` (a mirror,
/// or a local registry in a test), else Hermes' own `stable` on Docker Hub.
pub fn stable_ref() -> String {
    std::env::var("TALARIA_HERMES_STABLE_REF")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| STABLE_REF_DEFAULT.to_string())
}

/// The repository part of a reference as docker pulls it: Docker Hub's
/// without its host, any other registry's with it.
pub fn pull_repository(image: &ImageRef) -> String {
    if image.registry == "docker.io" {
        image.repository.clone()
    } else {
        format!("{}/{}", image.registry, image.repository)
    }
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub digest: String,
    pub repository: String,
    pub version: Option<String>,
    /// The digest was not on record before this check.
    pub new: bool,
}

/// Resolve the channel's tag and record the digest it names.
pub async fn check(pg: &PgPool) -> Result<Found, String> {
    let reference = stable_ref();
    let image = parse_image_ref(&reference)
        .ok_or_else(|| format!("the channel reference is not readable: {reference:.200}"))?;
    let repo = PublicRepo::open(&image).await?;
    let digest = repo.digest(&image.tag).await?;
    if !images::is_digest(&digest) {
        return Err("the registry named a digest that is not one".into());
    }
    let repository = pull_repository(&image);
    let known = images::record(pg, &digest)
        .await
        .map_err(|e| format!("the image record could not be read: {e}"))?;
    let version = match known.as_ref().and_then(|r| r.version.clone()) {
        Some(v) => Some(v),
        None => version_of(&repo, &digest).await,
    };
    let new = images::note_seen(pg, &digest, &repository, version.as_deref())
        .await
        .map_err(|e| format!("the image record could not be written: {e}"))?;
    Ok(Found {
        digest,
        repository,
        version,
        new,
    })
}

/// Which release tag shares `digest`: the newest release tags, HEADed one
/// at a time until one names it. Best-effort — a digest with no matching
/// tag still rolls, labelled by its digest.
async fn version_of(repo: &PublicRepo, digest: &str) -> Option<String> {
    let tags = match repo.tags().await {
        Ok(t) => t,
        Err(e) => {
            tracing::info!(
                "[hermes] the release tags could not be listed ({e}) — the digest goes unlabelled"
            );
            return None;
        }
    };
    for tag in newest_releases(&tags, VERSION_HEADS) {
        match repo.digest(&tag).await {
            Ok(d) if d == digest => return Some(tag),
            Ok(_) => {}
            Err(e) => tracing::info!("[hermes] {tag}: {e}"),
        }
    }
    None
}

/// The `n` highest `vX.Y.Z` tags, highest first; everything else (nightlies,
/// release candidates, desktop variants, `latest`) is not a release. Hermes'
/// older calendar tags (`v2026.9.24`) also read as `vX.Y.Z` and would sort
/// above every `v0.x` release, so a calendar tag (a four-digit first number)
/// comes after every other: measured 2026-11-03, the eight highest tags were
/// all calendar ones and none shared `stable`'s digest.
pub fn newest_releases(tags: &[String], n: usize) -> Vec<String> {
    let mut releases: Vec<(&String, (u32, u32, u32))> = tags
        .iter()
        .filter_map(|t| images::version_key(t).map(|k| (t, k)))
        .collect();
    let calendar = |k: &(u32, u32, u32)| k.0 >= 1000;
    releases.sort_by(|a, b| calendar(&a.1).cmp(&calendar(&b.1)).then(b.1.cmp(&a.1)));
    releases.dedup_by(|a, b| a.1 == b.1);
    releases
        .into_iter()
        .take(n)
        .map(|(t, _)| t.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_release_tags_are_matched_newest_first() {
        let tags: Vec<String> = [
            "latest",
            "stable",
            "main",
            "main-20261102",
            "v0.22.4",
            "v0.22.6",
            "v0.22.6-desktop",
            "rc.2-v0.23.0",
            "v0.9.10",
            "v0.22.10",
            "v1.0",
            // Hermes' older calendar scheme, still listed beside the new one.
            "v2026.9.24",
            "v2026.9.24-desktop",
            "v2026.10.1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            newest_releases(&tags, 3),
            vec!["v0.22.10", "v0.22.6", "v0.22.4"]
        );
        assert_eq!(
            newest_releases(&tags, 6)[4..],
            ["v2026.10.1".to_string(), "v2026.9.24".to_string()],
            "calendar tags come last, newest first"
        );
    }

    #[test]
    fn the_pull_repository_keeps_a_registry_host_but_not_docker_hubs() {
        let hub = parse_image_ref(STABLE_REF_DEFAULT).unwrap();
        assert_eq!(pull_repository(&hub), "nousresearch/hermes-agent");
        let local = parse_image_ref("127.0.0.1:5000/hermes-agent:stable").unwrap();
        assert_eq!(pull_repository(&local), "127.0.0.1:5000/hermes-agent");
        assert_eq!(local.tag, "stable");
    }
}
