// Message references — attaching KNOWLEDGE and ARTIFACTS to chat/channel
// messages, not just file uploads. A ref persists as a chip on the message
// (attachments jsonb, refType set) carrying a clipped copy of the content, so
// the model sees the material on the turn it was attached AND on every later
// history rebuild (queued turns, resumes, channel transcripts) without
// re-fetching. ACL-checked against the ATTACHER at attach time.
//
// Port of ui/src/server/refs.ts, whole — resolveRefs is what tasks.$id PUT
// stamps, and refBlocks is pure. The readers it leans on (kb, artifacts,
// kb_perms) crossed with it, read halves only.

use serde_json::Value;

use crate::artifacts::{artifact_to_markdown, get_artifact};
use crate::kb::perms::{ITEM_ARTIFACT, can_read, list_editors};
use crate::kb::{effective_doc_perms, get_doc};

/// A ref as the request names it (refs.ts MessageRef).
#[derive(Debug, Clone)]
pub struct MessageRef {
    pub ref_type: String, // 'kb-doc' | 'artifact'
    pub id: String,
}

/// The chip persisted into the message's attachments array (refs.ts RefChip).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefChip {
    pub id: String,
    pub filename: String,
    /// 'ref/kb-doc' | 'ref/artifact'
    pub mime: String,
    pub size: i64,
    pub ref_type: String,
    /// Clipped content the model reads; never rendered in the UI.
    pub content: String,
}

const CLIP: usize = 6_000;

fn clip(s: &str) -> String {
    if s.chars().count() > CLIP {
        let cut: String = s.chars().take(CLIP).collect();
        format!("{cut}\n[clipped]")
    } else {
        s.to_string()
    }
}

/// The attacher an ACL check runs against — id plus the author-string
/// fallback (email, else name) that ownerless items fall back to.
pub struct RefUser<'a> {
    pub id: &'a str,
    pub email: Option<&'a str>,
    pub name: Option<&'a str>,
}

impl RefUser<'_> {
    fn author(&self) -> Option<&str> {
        self.email.or(self.name)
    }
}

/// Resolve + ACL-check refs for a user. Unknown/forbidden refs are dropped
/// silently — attaching must never leak whether a private thing exists.
/// Read errors (a doc row that fails to load) are treated the same as a miss,
/// mirroring the TS `.catch(() => null)` legs.
pub async fn resolve_refs(
    pg: &sqlx::PgPool,
    user: &RefUser<'_>,
    refs: &[MessageRef],
) -> Result<Vec<RefChip>, sqlx::Error> {
    let mut chips: Vec<RefChip> = Vec::new();
    for r in refs.iter().take(3) {
        if r.ref_type == "kb-doc" {
            let Some(doc) = get_doc(pg, &r.id).await? else {
                continue;
            };
            let effective = effective_doc_perms(pg, &doc).await?;
            if !can_read(
                &effective.perms,
                Some(user.id),
                user.author(),
                &effective.grants,
            ) {
                continue;
            }
            let filename = if doc.title.is_empty() {
                "Untitled".to_string()
            } else {
                doc.title.clone()
            };
            chips.push(RefChip {
                id: doc.id.clone(),
                filename,
                mime: "ref/kb-doc".into(),
                size: 0,
                ref_type: "kb-doc".into(),
                content: clip(&doc.body),
            });
        } else {
            let Some(artifact) = get_artifact(pg, &r.id).await? else {
                continue;
            };
            let grants = list_editors(pg, ITEM_ARTIFACT, &artifact.id).await?;
            if !can_read(
                &crate::artifacts::guarded(&artifact),
                Some(user.id),
                user.author(),
                &grants,
            ) {
                continue;
            }
            let filename = if artifact.title.is_empty() {
                "Untitled".to_string()
            } else {
                artifact.title.clone()
            };
            chips.push(RefChip {
                id: artifact.id.clone(),
                filename,
                mime: "ref/artifact".into(),
                size: 0,
                ref_type: "artifact".into(),
                content: clip(&artifact_to_markdown(&artifact)),
            });
        }
    }
    Ok(chips)
}

/// The prompt block a message's ref chips contribute — used at send time and
/// by every history rebuild.
pub fn ref_blocks(attachments: &Value) -> String {
    let Some(items) = attachments.as_array() else {
        return String::new();
    };
    let refs: Vec<&Value> = items
        .iter()
        .filter(|a| {
            a.get("refType").and_then(Value::as_str).is_some()
                && a.get("content")
                    .and_then(Value::as_str)
                    .is_some_and(|c| !c.is_empty())
        })
        .collect();
    if refs.is_empty() {
        return String::new();
    }
    refs.iter()
        .map(|r| {
            let kind = if r["refType"] == "kb-doc" {
                "knowledge doc"
            } else {
                "artifact"
            };
            format!(
                "\n\n--- Attached {}: \"{}\" ({} {}) ---\n{}",
                kind,
                r["filename"].as_str().unwrap_or_default(),
                r["refType"].as_str().unwrap_or_default(),
                r["id"].as_str().unwrap_or_default(),
                r["content"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn clip_marks_the_cut() {
        let long = "x".repeat(6_001);
        let clipped = clip(&long);
        assert!(clipped.starts_with(&"x".repeat(6_000)));
        assert!(clipped.ends_with("\n[clipped]"));
        // At exactly the limit: no clip marker.
        assert_eq!(clip(&"x".repeat(6_000)), "x".repeat(6_000));
    }

    #[test]
    fn ref_blocks_renders_only_chip_entries_with_content() {
        let attachments = json!([
            { "id": "u-1", "filename": "pic.png", "mime": "image/png", "size": 10 },
            { "id": "d-1", "filename": "Spec", "refType": "kb-doc", "content": "the spec" },
            { "id": "a-1", "filename": "Grid", "refType": "artifact", "content": "| a |" },
            { "id": "a-2", "filename": "Empty", "refType": "artifact", "content": "" },
            "not an object",
        ]);
        assert_eq!(
            ref_blocks(&attachments),
            concat!(
                "\n\n--- Attached knowledge doc: \"Spec\" (kb-doc d-1) ---\nthe spec",
                "\n\n--- Attached artifact: \"Grid\" (artifact a-1) ---\n| a |"
            )
        );
        // No chips, or not an array → nothing.
        assert_eq!(ref_blocks(&json!([])), "");
        assert_eq!(ref_blocks(&json!(null)), "");
        assert_eq!(ref_blocks(&json!("nope")), "");
    }
}
