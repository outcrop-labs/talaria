// Artifact ↔ brain placement — the same routing control KB docs have.
//   auto   the artifact's normal flows (plan-doc / research activity indexing,
//          officialize → KB mirror) decide where it's retrievable
//   none   never indexed anywhere — scrub every copy
//   <id>   explicit custom-brain assignment: it lives ONLY there
// Privacy trumps routing: a private artifact never lands in a shared brain.
//
// THE TWO SINGLE-CALLER HELPERS. `targets_for_artifact` and `index_plan_doc`
// each have exactly one caller — this module — so they live here: a
// four-line query next to its only caller reads better than a module that
// exists for one fn.

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::artifacts::{Artifact, artifact_to_markdown};
use crate::retrieval::embed::EmbedDeps;
use crate::retrieval::index::{DocAcl, IndexDoc, index_document, unindex_document};
use crate::retrieval::qdrant::QdrantDeps;
use crate::retrieval::sources::{
    index_activity, index_personal, unindex_activity, unindex_personal,
};

/// An artifact's links, in the direction routing cares about.
struct ArtifactTarget {
    target_type: String,
    target_id: String,
}

/// The artifact's outgoing links.
async fn targets_for_artifact(pg: &PgPool, artifact_id: &str) -> Vec<ArtifactTarget> {
    // Fire-and-forget callers only: a failed read is no links, which routes
    // like an unlinked artifact rather than failing the re-placement.
    sqlx::query_as::<_, (String, String)>(
        "select target_type, target_id::text from artifact_links where artifact_id = $1::uuid",
    )
    .bind(artifact_id)
    .fetch_all(pg)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(target_type, target_id)| ArtifactTarget {
        target_type,
        target_id,
    })
    .collect()
}

/// Keep the activity brain current on a plan document (ACL: the plan's
/// owner). Respects the artifact's routing — 'none'/explicit-brain docs
/// stay out of the activity brain (this module owns those placements).
async fn index_plan_doc(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    doc: &Artifact,
    conversation_id: &str,
) {
    if !doc.rag_routing.is_empty() && doc.rag_routing != "auto" {
        return;
    }
    let _ = index_activity(
        pg,
        qd,
        ed,
        &IndexDoc {
            source_type: "plan-doc".into(),
            source_id: doc.id.clone(),
            title: Some(doc.title.clone()),
            text: format!("{}\n\n{}", doc.title, doc.body),
            payload: Some(obj(json!({
                "planId": conversation_id,
                "planOwnerId": doc.owner_user_id,
            }))),
            // Deep-linked: a retrieval hit is a pointer to ONE document, and
            // the id is right here.
            href: Some(format!("/artifacts?a={}", doc.id)),
        },
    )
    .await;
}

/// Re-place an artifact according to its routing. Idempotent; call after any
/// routing change (and the backfill/sweep call it for non-auto artifacts).
/// Never fails: every branch's writes are fire-and-forget, and a
/// re-placement that threw would fail a backfill page over one artifact.
pub async fn apply_artifact_routing(
    pg: &PgPool,
    qd: &QdrantDeps,
    ed: &EmbedDeps,
    artifact: &Artifact,
) {
    // Scrub explicit-brain copies first (re-routing must not leave stale
    // ones).
    let customs = sqlx::query_scalar::<_, String>(
        "select id::text from rag_collections where kind = 'custom'",
    )
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    for c in customs {
        let _ = unindex_document(pg, qd, &c, "artifact", &artifact.id).await;
    }

    if artifact.rag_routing == "none" {
        // Scrub the copies its auto flows may have created.
        let _ = unindex_activity(pg, qd, ed, "plan-doc", &artifact.id).await;
        let _ = unindex_activity(pg, qd, ed, "research", &artifact.id).await;
        if let Some(owner) = &artifact.owner_user_id {
            let _ = unindex_personal(pg, qd, owner, "research", &artifact.id).await;
        }
        return;
    }

    if artifact.rag_routing != "auto" {
        // Explicit brain: it lives only there — auto-flow copies go too.
        let _ = unindex_activity(pg, qd, ed, "plan-doc", &artifact.id).await;
        let _ = unindex_activity(pg, qd, ed, "research", &artifact.id).await;
        if let Some(owner) = &artifact.owner_user_id {
            let _ = unindex_personal(pg, qd, owner, "research", &artifact.id).await;
        }
        if artifact.visibility == "private" {
            return; // privacy trumps routing
        }
        let text = artifact_to_markdown(artifact);
        if text.trim().is_empty() {
            return; // files have no text body
        }
        let acl = DocAcl {
            visibility: artifact.visibility.clone(),
            owner_user_id: artifact.owner_user_id.clone(),
            space_id: None,
        };
        let _ = index_document(
            pg,
            qd,
            ed,
            &artifact.rag_routing,
            &IndexDoc {
                source_type: "artifact".into(),
                source_id: artifact.id.clone(),
                title: Some(artifact.title.clone()),
                text: format!("{}\n\n{}", artifact.title, text),
                // Item ACL — a custom brain holds items of mixed visibility,
                // so the document-scope filter re-checks this at query time.
                payload: Some(acl.to_map()),
                href: Some(format!("/artifacts?a={}", artifact.id)),
            },
        )
        .await;
        return;
    }

    // Back to auto: restore the flows that would have indexed it.
    let targets = targets_for_artifact(pg, &artifact.id).await;
    let plan = targets
        .iter()
        .find(|t| t.target_type == "plan")
        .map(|t| t.target_id.clone());
    let research = targets
        .iter()
        .find(|t| t.target_type == "research")
        .map(|t| t.target_id.clone());
    if let Some(plan_id) = plan {
        index_plan_doc(pg, qd, ed, artifact, &plan_id).await;
    } else if let Some(research_id) = research {
        // Personal research lives in the owner's private brain; org research
        // in the ambient index, marked orgWide so scopes match it.
        let doc = IndexDoc {
            source_type: "research".into(),
            source_id: artifact.id.clone(),
            title: Some(artifact.title.clone()),
            text: format!("{}\n\n{}", artifact.title, artifact.body),
            payload: Some(obj(if artifact.owner_user_id.is_some() {
                json!({ "runId": research_id })
            } else {
                json!({ "runId": research_id, "orgWide": true })
            })),
            href: Some(format!("/research/{research_id}")),
        };
        if artifact.visibility == "private" {
            if let Some(owner) = &artifact.owner_user_id {
                let _ = index_personal(pg, qd, ed, owner, &doc).await;
            }
        } else if artifact.visibility != "private" {
            let _ = index_activity(pg, qd, ed, &doc).await;
        }
    }
}

/// A `json!` object as the Map the IndexDoc payload wants — with
/// preserve_order this is insertion-ordered, the payload column's shape.
fn obj(v: Value) -> serde_json::Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => unreachable!("the literal is always an object"),
    }
}
