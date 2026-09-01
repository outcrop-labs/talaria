// The brief's artifact mirror — port of ui/src/server/daily-brief-artifact.ts.
//
// DERIVED, AND SAYS SO IN ITS OWN BODY. The log in `daily_brief_entries` is the
// truth; this is a rendering of it that happens to live somewhere a person can
// share. That direction is the whole design: the moment an artifact body became
// the source, "followable" would mean parsing structure back out of markdown,
// and the append-only guarantee would depend on a text rewrite being careful.
// Rewriting the body wholesale from the log on every append is boring and
// correct, and it costs one UPDATE.
//
// IT IS ALSO WHY NOTHING WAITS ON IT. `append_entries` fires this detached and
// logs a failure: a brief whose artifact did not re-render is a brief with a
// stale share link, which is recoverable on the next append. A brief that
// failed to append has lost part of somebody's day, which is not.
//
// VERSIONS COME FREE. `save_artifact` snapshots into `internal_versions` on
// every body change, so the artifact's history is a rough record of how the day
// accumulated — a second, human-shareable view of the same append-only shape.

use sqlx::PgPool;

use crate::artifacts::{SaveArtifactPatch, agent_category_folder, create_artifact, save_artifact};
use crate::daily_brief::types::{BRIEF_SECTIONS, BriefEntry, fold_entries};

const SECTION_TITLES: [(&str, &str); 4] = [
    ("action", "Needs you"),
    ("schedule", "Today's schedule"),
    ("comms", "Waiting on a reply"),
    ("highlights", "Worth knowing"),
];

fn section_title(section: &str) -> &'static str {
    SECTION_TITLES
        .iter()
        .find(|(key, _)| *key == section)
        .map(|(_, title)| *title)
        .unwrap_or("Worth knowing")
}

/// What the mirror needs off the brief row: (brief date, artifact id, artifact
/// folder, read seq, agent name, owner email, owner name).
type MirrorRow = (
    String,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Re-render the brief's artifact from the log. Creates it on first append.
/// Failures are the caller's to log — this returns them instead of printing,
/// because the caller (append_entries) is the one that knows it must not die.
pub async fn mirror_brief_artifact(
    pg: &PgPool,
    brief_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    let row: Option<MirrorRow> = sqlx::query_as(
        "select to_char(b.brief_date, 'YYYY-MM-DD'), b.artifact_id::text, a.folder_id::text, \
                b.read_seq::int8, b.agent_name, u.email, u.name \
         from daily_briefs b join users u on u.id = b.user_id \
         left join artifacts a on a.id = b.artifact_id \
         where b.id = $1::uuid and b.user_id = $2::uuid",
    )
    .bind(brief_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    let Some((brief_date, artifact_id, folder_id, read_seq, agent_name, owner_email, owner_name)) =
        row
    else {
        return Ok(());
    };

    let entries = crate::daily_brief::load_brief_entries(pg, brief_id).await?;

    let title = format!("Daily brief: {brief_date}");
    let actor = owner_email
        .as_deref()
        .or(owner_name.as_deref())
        .unwrap_or("talaria");
    let body = render_brief(
        &entries,
        &RenderContext {
            agent_name: agent_name.as_deref(),
            read_seq,
        },
    );
    // Briefs file with the rest of the agent's output — Agents/<agent>/Briefs —
    // not loose in the root of My Files. `agent_category_folder` never fails
    // (None = root), so a cabinet that cannot be built costs the brief its
    // folder, never its mirror.
    let cabinet = agent_category_folder(
        pg,
        agent_name.as_deref().unwrap_or("Your assistant"),
        "Briefs",
        actor,
    )
    .await;

    let Some(artifact_id) = artifact_id else {
        // PRIVATE, AND LEFT THAT WAY. A brief is one person's attention state —
        // their unread DMs, their approvals, their blocked work — so the only
        // acceptable default is the one that discloses nothing. `visibility` is
        // the owner's to change from the artifact surface, deliberately, by
        // hand.
        let created = create_artifact(
            pg,
            Some("doc"),
            Some(&title),
            actor,
            Some(user_id),
            cabinet.as_deref(),
        )
        .await?;
        sqlx::query("update daily_briefs set artifact_id = $2::uuid where id = $1::uuid and artifact_id is null")
            .bind(brief_id)
            .bind(&created.id)
            .execute(pg)
            .await?;
        // Re-read rather than trusting the insert: a concurrent append may have
        // won the race and created its own, in which case that one is the
        // mirror and this one is an orphan we simply stop writing to.
        let winner: Option<(Option<String>,)> =
            sqlx::query_as("select artifact_id::text from daily_briefs where id = $1::uuid")
                .bind(brief_id)
                .fetch_optional(pg)
                .await?;
        let target = winner.and_then(|(v,)| v).unwrap_or(created.id);
        save_artifact(
            pg,
            &target,
            SaveArtifactPatch {
                body: Some(&body),
                ..Default::default()
            },
            actor,
        )
        .await?;
        return Ok(());
    };
    // SELF-HEAL THE FOLDER, ONCE. A brief mirrored before cabinets existed sits
    // at the root; any later append files it. `folder_id ?? cabinet` — never
    // the reverse — so a person who deliberately moved their brief somewhere is
    // not fought on every append.
    save_artifact(
        pg,
        &artifact_id,
        SaveArtifactPatch {
            title: Some(Some(&title)),
            body: Some(&body),
            folder_id: Some(folder_id.as_deref().or(cabinet.as_deref())),
            visibility: None,
            ..Default::default()
        },
        actor,
    )
    .await?;
    Ok(())
}

pub struct RenderContext<'a> {
    pub agent_name: Option<&'a str>,
    pub read_seq: i64,
}

/// The log as markdown. Reads as the document does — lede, sections in document
/// order, then the day's timeline — because a person who shares this is sharing
/// what they were looking at, not a database dump.
pub fn render_brief(entries: &[BriefEntry], row: &RenderContext<'_>) -> String {
    let folded = fold_entries(entries.to_vec(), row.read_seq);
    let lede = entries.iter().find(|e| e.kind == "lede");
    let mut out: Vec<String> = Vec::new();

    if let Some(lede) = lede.filter(|e| !e.body.is_empty()) {
        out.push(lede.body.clone());
        out.push(String::new());
    }

    for section in BRIEF_SECTIONS {
        let in_section: Vec<_> = folded
            .lines
            .iter()
            .filter(|l| l.section == *section)
            .collect();
        if in_section.is_empty() {
            continue;
        }
        out.push(format!("## {}", section_title(section)));
        out.push(String::new());
        for line in in_section {
            out.push(render_line(line));
        }
        out.push(String::new());
    }

    if !folded.updates.is_empty() {
        out.push("## Timeline".into());
        out.push(String::new());
        // Oldest first here, unlike the surface. A shared document is read
        // top-to-bottom as a narrative; the live view leads with the newest
        // because its reader is checking what they missed.
        for update in folded.updates.iter().rev() {
            // HH:MM of the update's ISO `at` — the string is this system's own
            // UTC spelling, so the slice is the clock, no re-parse needed.
            let at = update.at.get(11..16).unwrap_or("");
            let headline = update
                .note
                .clone()
                .unwrap_or_else(|| format!("{} update(s)", update.entries.len()));
            out.push(format!("**{at}**: {headline}"));
            for entry in &update.entries {
                let verb = match entry.kind.as_str() {
                    "resolved" => "resolved",
                    "change" => "changed",
                    _ => "new",
                };
                out.push(format!("- _{verb}_: {}", entry.title));
            }
            out.push(String::new());
        }
    }

    out.push("---".into());
    out.push(format!(
        "_Written by {}. Appended to through the day; never rewritten._",
        row.agent_name.unwrap_or("your assistant")
    ));
    out.join("\n")
}

fn render_line(line: &crate::daily_brief::types::BriefLine) -> String {
    let e = &line.current;
    let label = match &e.source_href {
        Some(href) => format!("[{}]({href})", e.title),
        None => e.title.clone(),
    };
    let head = if line.resolved {
        format!("- ~~{label}~~")
    } else {
        format!("- **{label}**")
    };
    let badge_label = e
        .badge
        .as_ref()
        .and_then(|b| b.get("label"))
        .and_then(|l| l.as_str());
    let tags = [e.status_label.as_deref(), badge_label]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let tags = if tags.is_empty() {
        String::new()
    } else {
        format!(" `{tags}`")
    };
    let body = if !e.body.is_empty() && !line.resolved {
        format!("\n  {}", e.body)
    } else {
        String::new()
    };
    format!("{head}{tags}{body}")
}
