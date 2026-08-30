// The two personal-agent read helpers other surfaces borrow — port of
// personalityOf and ownsAgent from ui/src/server/personal-agent.ts. The
// create/update flow around them (handles, soul scaffolds, seed tasks) is
// write-plane and crosses with its own batch; /api/history needs exactly
// these two today.

use sqlx::PgPool;

// The owner edits one marked section of the soul; the rest of the soul (role
// scaffold, guardrails) stays out of their way. Markers are HTML comments so
// they're invisible wherever the soul renders as markdown.
const PERSONA_START: &str = "<!-- talaria:personality -->";
const PERSONA_END: &str = "<!-- /talaria:personality -->";

/// The marked personality section of a soul, trimmed — null when the markers
/// are absent, out of order, or bracket only whitespace. Version history
/// serves this for `kind=personality`, so the null/empty distinction is the
/// wire contract (`personalityOf(v.soul) ?? ''`).
pub fn personality_of(soul: &str) -> Option<String> {
    let m = soul.find(PERSONA_START)?;
    let e = soul.find(PERSONA_END)?;
    if e < m {
        return None;
    }
    let inner = soul[m + PERSONA_START.len()..e].trim();
    (!inner.is_empty()).then(|| inner.to_string())
}

/// Does this user own the agent (by slug or def id)? Used to open selected
/// admin surfaces (skills, memory, start/stop) to an assistant's owner.
/// Fail-closed: any error — e.g. a non-uuid defId — reads as no, exactly
/// like the TS `catch { return false }` around the query.
pub async fn owns_agent(
    pg: &PgPool,
    user_id: &str,
    slug: Option<&str>,
    def_id: Option<&str>,
) -> bool {
    // JS truthiness in the ref: an empty slug falls through to the defId
    // branch, an empty defId queries nothing — neither is a lookup.
    let slug = slug.filter(|s| !s.is_empty());
    let def_id = def_id.filter(|s| !s.is_empty());
    let found = if let Some(slug) = slug {
        sqlx::query_scalar::<_, i32>(
            "select 1 from agent_defs \
             where owner_user_id = $1::uuid and slug = $2",
        )
        .bind(user_id)
        .bind(slug)
        .fetch_optional(pg)
        .await
    } else if let Some(def_id) = def_id {
        sqlx::query_scalar::<_, i32>(
            "select 1 from agent_defs \
             where owner_user_id = $1::uuid and id = $2::uuid",
        )
        .bind(user_id)
        .bind(def_id)
        .fetch_optional(pg)
        .await
    } else {
        return false;
    };
    found.map(|row| row.is_some()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personality_extraction() {
        let soul = "## Role\nYou help.\n\n<!-- talaria:personality -->\nWarm and brief.\n<!-- /talaria:personality -->\n";
        assert_eq!(personality_of(soul).as_deref(), Some("Warm and brief."));
    }

    #[test]
    fn personality_markers_missing_or_swapped() {
        assert_eq!(personality_of("no markers at all"), None);
        assert_eq!(
            personality_of("<!-- /talaria:personality -->x<!-- talaria:personality -->"),
            None
        );
        // end marker alone, start marker alone
        assert_eq!(personality_of("<!-- /talaria:personality -->"), None);
        assert_eq!(personality_of("<!-- talaria:personality -->"), None);
    }

    #[test]
    fn personality_empty_section_is_null() {
        assert_eq!(
            personality_of("a<!-- talaria:personality --><!-- /talaria:personality -->b"),
            None
        );
        assert_eq!(
            personality_of("a<!-- talaria:personality --> \n\t <!-- /talaria:personality -->b"),
            None
        );
    }

    #[test]
    fn personality_first_marker_pair_wins() {
        // indexOf takes the FIRST occurrence of each marker — a second start
        // marker inside the section is content, not a delimiter.
        let soul = "<!-- talaria:personality -->one<!-- talaria:personality -->two<!-- /talaria:personality -->";
        assert_eq!(
            personality_of(soul).as_deref(),
            Some("one<!-- talaria:personality -->two")
        );
    }

    #[test]
    fn personality_trims_both_sides() {
        let soul = "<!-- talaria:personality -->\n\n  keep this  \n\n<!-- /talaria:personality -->";
        assert_eq!(personality_of(soul).as_deref(), Some("keep this"));
    }
}
