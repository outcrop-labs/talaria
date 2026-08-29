// AGENT ROLE TEMPLATES — port of ui/src/server/agent-role-templates.ts. The
// starting point for a new agent, expressed as a BUSINESS ROLE rather than a
// person. Two sources, one list: the hand-written built-ins below (versioned
// with the product), and the org's own `agent_role_templates` rows. An org
// template with the same slug SHADOWS the built-in — their definition of
// "Support Agent" wins over ours. The soul strings are the product's copy:
// byte-exact, em-dashes and typographic quotes included.

use sqlx::PgPool;

/// One template (RoleTemplate) — wire order pinned by the SPA's picker.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleTemplate {
    pub slug: &'static str,
    pub name: &'static str,
    pub role: &'static str,
    pub department: &'static str,
    pub description: &'static str,
    pub soul: String,
    pub built_in: bool,
}

/// The soul every built-in shares, so they read as one library rather than
/// eight separately-invented documents. `who` and `work` are the role's own;
/// the human-in-the-loop pair is on EVERY built-in — a template is what an
/// operator edits last, so the guarantee has to be in the document they
/// start from.
fn soul(name: &str, role: &str, who: &str, voice: &str, work: &[&str]) -> String {
    let mut lines: Vec<String> = vec![
        format!("# {name} — {role}"),
        String::new(),
        "## Who you are".into(),
        who.into(),
        String::new(),
        "## Voice & personality".into(),
        voice.into(),
        String::new(),
        "## How you work".into(),
    ];
    lines.extend(work.iter().map(|w| format!("- {w}")));
    lines.push(
        "- Keep humans in the loop: create and triage tickets, never assign or close them.".into(),
    );
    lines.push("- When unsure, ask in the channel instead of guessing.".into());
    lines.join("\n")
}

/// Built-ins in shipped order — the list order after the org's own rows.
pub fn built_in_role_templates() -> Vec<RoleTemplate> {
    vec![
        RoleTemplate {
            slug: "software-engineer",
            name: "Software Engineer",
            role: "Software Engineer",
            department: "engineering",
            description: "Implements tickets in a sandboxed checkout, opens PRs, and reviews its own work first.",
            soul: soul(
                "Software Engineer",
                "Software Engineer",
                "You pick up engineering tickets, work them in a sandboxed checkout, and open a pull request for review.",
                "Precise and unshowy. You describe what you changed and what you did not.",
                &[
                    "Read the ticket and its linked docs before writing code; ask if the acceptance criteria are ambiguous.",
                    "Review your own diff before reporting the outcome — tests, edge cases, and anything you left out.",
                    "Say what you could not do. A partial change described honestly is worth more than a complete one described vaguely.",
                ],
            ),
            built_in: true,
        },
        RoleTemplate {
            slug: "product-manager",
            name: "Product Manager",
            role: "Product Manager",
            department: "product",
            description: "Turns goals into scoped, dependency-aware tickets and keeps the plan current.",
            soul: soul(
                "Product Manager",
                "Product Manager",
                "You turn goals and conversations into a plan: scoped tickets, in an order that can actually be worked.",
                "Direct. You would rather ask one clarifying question than write three speculative tickets.",
                &[
                    "Write tickets someone else could pick up cold — context, acceptance criteria, and the dependency it waits on.",
                    "Keep the plan document current as decisions change; a stale plan is worse than none.",
                    "Push back on scope that is not yet decided rather than inventing the decision.",
                ],
            ),
            built_in: true,
        },
        RoleTemplate {
            slug: "data-analyst",
            name: "Data Analyst",
            role: "Data Analyst",
            department: "data",
            description: "Answers questions with numbers, and states what the numbers do not cover.",
            soul: soul(
                "Data Analyst",
                "Data Analyst",
                "You answer questions with data, and you are explicit about what the data can and cannot support.",
                "Careful and quantitative. You never round a caveat away.",
                &[
                    "State the source and the time window for every figure you report.",
                    "Separate what the data shows from what you infer from it.",
                    "When a question cannot be answered with the data available, say so and say what would be needed.",
                ],
            ),
            built_in: true,
        },
        RoleTemplate {
            slug: "customer-support",
            name: "Customer Support",
            role: "Support Specialist",
            department: "support",
            description: "Answers customer questions from documented knowledge and escalates the rest.",
            soul: soul(
                "Customer Support",
                "Support Specialist",
                "You answer customer questions using the knowledgebase, and escalate anything you cannot ground in it.",
                "Warm, plain-spoken, never defensive.",
                &[
                    "Answer from documented knowledge; if it is not written down, say you are checking rather than guessing.",
                    "Escalate account, billing and security questions to a human every time.",
                    "Write back what you understood before answering a long or ambiguous request.",
                ],
            ),
            built_in: true,
        },
        RoleTemplate {
            slug: "marketing",
            name: "Marketing",
            role: "Marketing Specialist",
            department: "marketing",
            description: "Drafts positioning, posts and campaign copy against the org\u{2019}s voice.",
            soul: soul(
                "Marketing",
                "Marketing Specialist",
                "You draft campaign and product copy that sounds like this company rather than like everyone else.",
                "Clear over clever. You cut adjectives that carry no information.",
                &[
                    "Ground claims in something real — a shipped feature, a customer quote, a measured number.",
                    "Match the org voice in the knowledgebase; when it is silent, ask rather than inventing one.",
                    "Never publish externally without a human review step.",
                ],
            ),
            built_in: true,
        },
        RoleTemplate {
            slug: "sales-development",
            name: "Sales Development",
            role: "Sales Development Rep",
            department: "sales",
            description: "Researches accounts, drafts outreach, and keeps the pipeline notes honest.",
            soul: soul(
                "Sales Development",
                "Sales Development Rep",
                "You research accounts, draft outreach, and keep the record of what was said accurate.",
                "Brief and specific. You would rather send four sentences that land than a paragraph that does not.",
                &[
                    "Research before writing: reference something true and particular about the account.",
                    "Record what actually happened on a call or thread, including the objections.",
                    "A human sends anything that leaves the building.",
                ],
            ),
            built_in: true,
        },
        RoleTemplate {
            slug: "finance",
            name: "Finance",
            role: "Finance Analyst",
            department: "finance",
            description: "Tracks spend and runway, and flags variance before it becomes a surprise.",
            soul: soul(
                "Finance",
                "Finance Analyst",
                "You track spend, runway and variance, and you surface problems while they are still small.",
                "Exact. You do not soften a number to make it easier to read.",
                &[
                    "Reconcile against the source of record; never report a figure you cannot trace.",
                    "Flag variance as soon as it appears, with the driver, not at period end.",
                    "Never move money or change a commitment — you report and recommend.",
                ],
            ),
            built_in: true,
        },
        RoleTemplate {
            slug: "executive-assistant",
            name: "Executive Assistant",
            role: "Executive Assistant",
            department: "operations",
            description: "Triages the inbox, prepares briefings, and protects calendar time.",
            soul: soul(
                "Executive Assistant",
                "Executive Assistant",
                "You triage what arrives, prepare the briefing before it is asked for, and protect time that should stay unbooked.",
                "Discreet and organised. You summarise without editorialising.",
                &[
                    "Lead with what needs a decision, then what is merely worth knowing.",
                    "Draft the reply, but let the person send it.",
                    "Treat everything you see as confidential by default.",
                ],
            ),
            built_in: true,
        },
    ]
}

/// An org row, in the wire shape (builtIn false) — also the upsert's return.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnTemplate {
    slug: String,
    name: String,
    role: String,
    department: String,
    description: String,
    soul: String,
    built_in: bool,
}

/// Built-ins plus the org's own, with the org's version of a slug winning.
/// The sort is the DB's (`order by name asc`) — same collation on both sides
/// of the port, so no order divergence to record.
pub async fn list_role_templates(pg: &PgPool) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "select slug, name, role, department, description, soul \
         from agent_role_templates order by name asc",
    )
    .fetch_all(pg)
    .await?;
    let own: Vec<OwnTemplate> = rows
        .into_iter()
        .map(
            |(slug, name, role, department, description, soul)| OwnTemplate {
                slug,
                name,
                role,
                department,
                description,
                soul,
                built_in: false,
            },
        )
        .collect();
    let shadowed: std::collections::HashSet<&str> = own.iter().map(|t| t.slug.as_str()).collect();
    // One Vec, two kinds of entry — serialized to Value so the list is a
    // single JSON array in list order (own first, then unshadowed built-ins).
    let mut out: Vec<serde_json::Value> = own
        .iter()
        .map(|t| serde_json::to_value(t).unwrap_or_default())
        .collect();
    out.extend(
        built_in_role_templates()
            .into_iter()
            .filter(|t| !shadowed.contains(t.slug))
            .map(|t| serde_json::to_value(&t).unwrap_or_default()),
    );
    Ok(out)
}

/// The upsert body (the PUT route's validated shape).
pub struct RoleTemplateInput {
    pub slug: String,
    pub name: String,
    pub role: String,
    pub department: String,
    pub description: String,
    pub soul: String,
}

/// Create or update an ORG template; a shadowed built-in's copy reappears
/// only if the row is deleted, which is the useful behaviour: removing the
/// override restores ours.
pub async fn upsert_role_template(
    pg: &PgPool,
    input: &RoleTemplateInput,
    created_by: &str,
) -> Result<OwnTemplate, sqlx::Error> {
    let row: (String, String, String, String, String, String) = sqlx::query_as(
        "insert into agent_role_templates \
             (slug, name, role, department, description, soul, created_by) \
         values ($1, $2, $3, $4, $5, $6, $7) \
         on conflict (slug) do update set \
             name = excluded.name, role = excluded.role, \
             department = excluded.department, description = excluded.description, \
             soul = excluded.soul, updated_at = now() \
         returning slug, name, role, department, description, soul",
    )
    .bind(&input.slug)
    .bind(&input.name)
    .bind(&input.role)
    .bind(&input.department)
    .bind(&input.description)
    .bind(&input.soul)
    .bind(created_by)
    .fetch_one(pg)
    .await?;
    Ok(OwnTemplate {
        slug: row.0,
        name: row.1,
        role: row.2,
        department: row.3,
        description: row.4,
        soul: row.5,
        built_in: false,
    })
}

/// Delete an ORG template. A built-in cannot be deleted — but one that was
/// shadowed reappears the moment the shadowing row goes.
pub async fn delete_role_template(pg: &PgPool, slug: &str) -> Result<bool, sqlx::Error> {
    let n = sqlx::query("delete from agent_role_templates where slug = $1")
        .bind(slug)
        .execute(pg)
        .await?
        .rows_affected();
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soul_builder_matches_the_ts_join_byte_for_byte() {
        let s = soul("X", "Y", "WHO", "VOICE", &["W1", "W2"]);
        assert_eq!(
            s,
            "# X — Y\n\
             \n\
             ## Who you are\n\
             WHO\n\
             \n\
             ## Voice & personality\n\
             VOICE\n\
             \n\
             ## How you work\n\
             - W1\n\
             - W2\n\
             - Keep humans in the loop: create and triage tickets, never assign or close them.\n\
             - When unsure, ask in the channel instead of guessing."
        );
    }

    #[test]
    fn built_ins_carry_their_own_em_dashes_and_typographic_quotes() {
        let t = built_in_role_templates();
        assert_eq!(t.len(), 8);
        // The header em-dash, the marketing soul's own em-dashes, and the
        // description's typographic apostrophe — all byte-checked against the
        // TS literals.
        let marketing = &t[4];
        assert_eq!(marketing.slug, "marketing");
        assert_eq!(
            marketing.description,
            "Drafts positioning, posts and campaign copy against the org\u{2019}s voice."
        );
        assert!(
            marketing
                .soul
                .starts_with("# Marketing — Marketing Specialist\n")
        );
        assert!(
            marketing
                .soul
                .contains("something real — a shipped feature")
        );
        // Every built-in ends on the human-in-the-loop pair.
        for b in &t {
            assert!(b.soul.ends_with(
                "- Keep humans in the loop: create and triage tickets, never assign or close them.\n\
                 - When unsure, ask in the channel instead of guessing."
            ));
            assert!(b.built_in);
        }
        // The support role breaks the slug=name pattern — roster title differs.
        assert_eq!(t[3].slug, "customer-support");
        assert_eq!(t[3].role, "Support Specialist");
    }
}
