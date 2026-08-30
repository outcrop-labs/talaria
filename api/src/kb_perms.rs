// Knowledgebase permissions — shared by docs and folders (spaces).
//
//   visibility  (read):  private → owner only · org → members · public → link
//   edit_policy (write):  owner → owner only · org → any reader · restricted →
//                         owner + an explicit editor list (users and/or agents)
//
// Agents never get implicit edit rights: even under 'org' they must be named in
// the editor list. That keeps automated edits deliberate.
//
// Port of ui/src/server/kb-perms.ts, read half: the grant list and the two
// predicates the refs cone checks. The edit predicates (canEditHuman/
// canEditAgent), governance, and the setEditors write land with the kb and
// artifacts planes in batch 5 — one ACL table, extended in place, never forked.

use sqlx::PgPool;

/// What a grant can hang off (`kb_editors.item_type`). `artifact-folder`
/// joined the set when Files got shareable folders — the access model is
/// identical, so it reuses the same table and the same checks rather than
/// growing a second one.
pub const ITEM_DOC: &str = "doc";
pub const ITEM_SPACE: &str = "space";
pub const ITEM_ARTIFACT: &str = "artifact";

/// An explicit editor-list entry (kb-perms.ts EditorGrant). `role` is
/// 'viewer' | 'editor' — kept as the raw string; the table's enum is the
/// authority and these flow through untouched.
#[derive(Debug, Clone)]
pub struct EditorGrant {
    pub principal_type: String,
    pub principal_id: String,
    pub role: String,
}

/// Shared shape both docs and spaces expose for permission checks.
#[derive(Debug, Clone)]
pub struct Guarded {
    pub owner_user_id: Option<String>,
    pub created_by: Option<String>,
    pub visibility: String,
    pub edit_policy: String,
}

pub async fn list_editors(
    pg: &PgPool,
    item_type: &str,
    item_id: &str,
) -> Result<Vec<EditorGrant>, sqlx::Error> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "select principal_type, principal_id::text, role \
         from kb_editors where item_type = $1 and item_id = $2::uuid",
    )
    .bind(item_type)
    .bind(item_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(principal_type, principal_id, role)| EditorGrant {
            principal_type,
            principal_id,
            role,
        })
        .collect())
}

/// setEditors — replace an item's whole editor list in one transaction: delete,
/// then upsert each grant. Research's write step is the caller that needs it
/// (sharing a run grants the members editor on the report doc — the only way
/// anyone else sees a private run's artifact); the sharing surfaces themselves
/// cross with the kb/artifacts planes in batch 5, extending this in place.
pub async fn set_editors(
    pg: &PgPool,
    item_type: &str,
    item_id: &str,
    grants: &[EditorGrant],
) -> Result<(), sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("delete from kb_editors where item_type = $1 and item_id = $2::uuid")
        .bind(item_type)
        .bind(item_id)
        .execute(&mut *tx)
        .await?;
    for g in grants {
        sqlx::query(
            "insert into kb_editors (item_type, item_id, principal_type, principal_id, role) \
             values ($1, $2::uuid, $3, $4::uuid, $5) \
             on conflict (item_type, item_id, principal_type, principal_id) \
             do update set role = excluded.role",
        )
        .bind(item_type)
        .bind(item_id)
        .bind(&g.principal_type)
        .bind(&g.principal_id)
        .bind(if g.role == "editor" {
            "editor"
        } else {
            "viewer"
        })
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// True if the human (by user id / author string) owns the item. Falls back to
/// the author string for items created before owner ids were tracked. Only the
/// owner may re-share (change visibility / edit policy / editor list).
pub fn is_owner(item: &Guarded, user_id: Option<&str>, author: Option<&str>) -> bool {
    if let Some(owner) = item.owner_user_id.as_ref() {
        return user_id.is_some_and(|u| owner == u);
    }
    author.is_some_and(|a| item.created_by.as_deref() == Some(a))
}

/// Can this signed-in human read the item? Owner, org/public visibility, or any
/// explicit grant (viewer or editor) on a private item.
pub fn can_read(
    item: &Guarded,
    user_id: Option<&str>,
    author: Option<&str>,
    grants: &[EditorGrant],
) -> bool {
    let Some(user_id) = user_id else { return false };
    if item.visibility != "private" {
        return true; // org or public → any member
    }
    if is_owner(item, Some(user_id), author) {
        return true;
    }
    grants
        .iter()
        .any(|g| g.principal_type == "user" && g.principal_id == user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guarded(visibility: &str, owner: Option<&str>) -> Guarded {
        Guarded {
            owner_user_id: owner.map(str::to_string),
            created_by: Some("creator@x.com".into()),
            visibility: visibility.into(),
            edit_policy: "owner".into(),
        }
    }

    fn grant(ptype: &str, pid: &str, role: &str) -> EditorGrant {
        EditorGrant {
            principal_type: ptype.into(),
            principal_id: pid.into(),
            role: role.into(),
        }
    }

    #[test]
    fn can_read_never_anonymous() {
        // No user id → nothing is readable, not even public.
        assert!(!can_read(&guarded("public", None), None, None, &[]));
    }

    #[test]
    fn org_and_public_are_member_readable() {
        for v in ["org", "public"] {
            assert!(
                can_read(&guarded(v, Some("u-1")), Some("u-2"), None, &[]),
                "{v}"
            );
        }
    }

    #[test]
    fn private_reads_need_ownership_or_a_grant() {
        let g = guarded("private", Some("u-1"));
        // A stranger with no grant: no.
        assert!(!can_read(&g, Some("u-2"), None, &[]));
        // The owner: yes.
        assert!(can_read(&g, Some("u-1"), None, &[]));
        // Any explicit grant — viewer counts as much as editor.
        assert!(can_read(
            &g,
            Some("u-2"),
            None,
            &[grant("user", "u-2", "viewer")]
        ));
        // A grant naming someone else: no.
        assert!(!can_read(
            &g,
            Some("u-2"),
            None,
            &[grant("user", "u-3", "editor")]
        ));
        // Agent grants never satisfy a human read.
        assert!(!can_read(
            &g,
            Some("u-2"),
            None,
            &[grant("agent", "u-2", "editor")]
        ));
    }

    #[test]
    fn ownership_falls_back_to_the_author_string() {
        // Ownerless item created before owner ids were tracked: the author
        // string (email or name) is the owner.
        let mut g = guarded("private", None);
        g.created_by = Some("whoever@x.com".into());
        assert!(can_read(&g, Some("u-1"), Some("whoever@x.com"), &[]));
        assert!(!can_read(&g, Some("u-1"), Some("someone-else"), &[]));
        // But once an owner id exists, ONLY that id owns it — the author
        // string no longer matters.
        let owned = guarded("private", Some("u-9"));
        assert!(!can_read(&owned, Some("u-1"), Some("creator@x.com"), &[]));
        assert!(can_read(&owned, Some("u-9"), None, &[]));
    }
}
