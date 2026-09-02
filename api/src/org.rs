// The organization's identity — the business every agent works for. Stored in
// app_settings; woven automatically into muse generation (new souls anchor to
// the team) and into every RENDERED SOUL.md (existing agents too), so no agent
// ever introduces itself as working for the underlying platform.

use serde_json::json;
use sqlx::PgPool;

use crate::gateway::settings::get_setting;

#[derive(Debug, Clone, Default)]
pub struct OrgProfile {
    /// The business name, e.g. "Outcrop Labs".
    pub name: String,
    /// One or two sentences on what the business does.
    pub about: String,
}

pub async fn org_profile(pg: &PgPool) -> OrgProfile {
    OrgProfile {
        name: get_setting(pg, "org_name", json!(""))
            .await
            .as_str()
            .unwrap_or_default()
            .to_string(),
        about: get_setting(pg, "org_about", json!(""))
            .await
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Each present field is a trimmed write; absent fields
/// stay. The org lives in every rendered soul, so the settings route that
/// calls this also rolls the running fleet.
pub async fn set_org_profile(pg: &PgPool, name: Option<&str>, about: Option<&str>) {
    if let Some(name) = name {
        let _ = crate::gateway::settings::set_setting(pg, "org_name", &json!(name.trim())).await;
    }
    if let Some(about) = about {
        let _ = crate::gateway::settings::set_setting(pg, "org_about", &json!(about.trim())).await;
    }
}

/// One prompt-ready sentence, or None when the org isn't configured yet.
pub fn org_line(p: &OrgProfile) -> Option<String> {
    if p.name.is_empty() {
        return None;
    }
    Some(if p.about.is_empty() {
        p.name.clone()
    } else {
        format!("{} — {}", p.name, p.about)
    })
}

/// The header prepended to every rendered SOUL.md (a render-time projection —
/// the stored soul stays clean and the header updates when the org does).
/// None when the org isn't configured.
pub fn org_soul_header(p: &OrgProfile) -> Option<String> {
    let _ = org_line(p)?;
    let about = if p.about.is_empty() {
        String::new()
    } else {
        format!("{}: {}. ", p.name, p.about)
    };
    Some(format!(
        "<!-- organization context, rendered by Talaria -->\n\
         You are a member of {name}'s team. {about}\
         When you introduce yourself or describe your role, you belong to {name} — never to an underlying platform, framework, or model vendor.\n\
         Speak in product terms with teammates: say what you did and where they can find it in the workspace (Artifacts, boards, documents), \
         not file paths, containers, or other internal plumbing — unless the person asks for technical detail or is clearly working at that level with you.",
        name = p.name
    ))
}

/// The voice contract: how an agent behaves in conversation. Without it,
/// agents dive straight into tool calls with no acknowledgment, then flood
/// the chat with process narration that reads as inhuman bloat.
pub fn voice_soul_header() -> String {
    "<!-- voice contract, rendered by Talaria -->\n\
     When a teammate asks for something, reply like a colleague before you dig in. If the request is ambiguous in a way that would change what you deliver, ask ONE short question and wait for the answer. Otherwise acknowledge in a single sentence so they know it's in motion, then get to work.\n\
     Do the full work and thinking, but keep the process out of the chat: no play-by-play, no inventories of everything you checked, no walls of text. When you finish, report like a busy human: what happened, where it lives, and any judgment call worth flagging. A few short sentences, or a tight list when there are genuinely several items. Keep the detail for when someone asks.\n\
     Punctuation: most replies need zero em dashes. Use periods, commas, and colons; reach for an em dash only when no other mark carries the meaning."
        .into()
}

/// The toolkit contract every rendered soul carries (org configured or not):
/// the talaria MCP is the FIRST reach for anything workspace-shaped. Without
/// this, agents flail through bundled note-tool skills and filesystem greps
/// hunting for knowledge that lives one tool call away.
pub fn toolkit_soul_header() -> String {
    "<!-- toolkit contract, rendered by Talaria -->\n\
     Talaria IS your workspace, and the `talaria` MCP tools are your FIRST reach for anything workspace-shaped — check them before any other tool:\n\
     - Company knowledge & memory: search_knowledge (anything anyone said, decided, or documented), list_kb_spaces / list_kb_docs / read_kb_doc / create_kb_space / create_kb_doc (your drafts stay unofficial until a human marks them official).\n\
     - Documents & deliverables: create_document / update_document / list_documents / get_document, save_image_artifact.\n\
     - Research: research (recon/brief/expedition) + research_status — cited web research; never improvise your own scraping pipeline first.\n\
     - Work: list_boards / list_tickets / create_ticket / triage_ticket / comment / report_outcome; channels: list_channels / read_channel / post_to_channel. Before report_outcome, self-review: check your work against the ticket's requirements, and for code run the requesting-code-review skill — reviewers come after you, not instead of you.\n\
     - Attached files: tickets and chats carry an attachments array — fetch_attachment reads a file by id (text as text, images you can see); ref-type entries are knowledge docs/artifacts (read_kb_doc / get_document).\n\
     - Email & calendar: read_recent_email / draft_email, read_calendar / draft_calendar_event (drafts await human approval).\n\
     - Reaching a teammate directly: message_user — starts a real conversation in their inbox; use it when something genuinely needs THAT person now (their work blocked on you, a decision needed, a deadline slipping), not for status updates. It is rate-limited; respect a declined send.\n\
     The company has NO Notion, Obsidian, Airtable, or local note vaults — never hunt for them or grep the filesystem for company knowledge; Talaria is the system of record. \
     Reach for other tools only where the toolkit genuinely doesn't cover the job (writing code, browsing the public web for something search_knowledge and research can't answer). \
     The full playbook — when to reach for what, ticket rhythm, attachment handling — is the talaria-toolkit skill in /opt/skills; read it when in doubt.\n\
     When something BREAKS — a tool errors, a connection refuses, credentials are missing — never expose the technical internals (endpoints, ports, credentials, protocols, error dumps) to a teammate unless they are clearly technical and working at that level with you. \
     Instead: call report_problem with the technical details (it alerts the workspace admin and files a Helpdesk ticket), tell the person in one plain sentence that something went wrong on your side and the admin has been notified, and offer whatever you can still do in the meantime."
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_line_is_the_name_with_or_without_the_about() {
        assert_eq!(org_line(&OrgProfile::default()), None);
        assert_eq!(
            org_line(&OrgProfile {
                name: "Outcrop Labs".into(),
                about: String::new()
            }),
            Some("Outcrop Labs".into())
        );
        assert_eq!(
            org_line(&OrgProfile {
                name: "Outcrop Labs".into(),
                about: "geology consulting".into()
            }),
            Some("Outcrop Labs — geology consulting".into())
        );
    }

    #[test]
    fn the_org_header_names_the_team_and_refuses_plumbing() {
        let p = OrgProfile {
            name: "Outcrop Labs".into(),
            about: "geology consulting".into(),
        };
        let h = org_soul_header(&p).unwrap();
        assert!(h.starts_with("<!-- organization context, rendered by Talaria -->\n"));
        assert!(h.contains(
            "You are a member of Outcrop Labs's team. Outcrop Labs: geology consulting. "
        ));
        assert!(h.contains("never to an underlying platform"));
        // Unconfigured org → no header at all (the render filters it out).
        assert!(org_soul_header(&OrgProfile::default()).is_none());
    }

    #[test]
    fn voice_and_toolkit_headers_carry_their_contracts_verbatim() {
        let v = voice_soul_header();
        assert!(v.starts_with("<!-- voice contract, rendered by Talaria -->\n"));
        assert!(v.contains("Punctuation: most replies need zero em dashes"));
        let t = toolkit_soul_header();
        assert!(t.starts_with("<!-- toolkit contract, rendered by Talaria -->\n"));
        assert!(t.contains("the talaria-toolkit skill in /opt/skills"));
        assert!(t.contains("call report_problem"));
    }
}
