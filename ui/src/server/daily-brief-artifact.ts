// The brief's artifact mirror — share, export, public link, version history.
//
// DERIVED, AND SAYS SO IN ITS OWN BODY. The log in `daily_brief_entries` is the
// truth; this is a rendering of it that happens to live somewhere a person can
// share. That direction is the whole design: the moment an artifact body became
// the source, "followable" would mean parsing structure back out of markdown,
// and the append-only guarantee would depend on a text rewrite being careful.
// Rewriting the body wholesale from the log on every append is boring and
// correct, and it costs one UPDATE.
//
// IT IS ALSO WHY NOTHING WAITS ON IT. `appendEntries` fires this detached and
// logs a failure: a brief whose artifact did not re-render is a brief with a
// stale share link, which is recoverable on the next append. A brief that
// failed to append has lost part of somebody's day, which is not.
//
// VERSIONS COME FREE. `saveArtifact` snapshots into `internal_versions` on
// every body change, so the artifact's history is a rough record of how the day
// accumulated — a second, human-shareable view of the same append-only shape.
import { db } from './db/pg'
import { createArtifact, saveArtifact } from './artifacts'
import { foldEntries } from './daily-brief-fold'
import type { BriefEntry } from './daily-brief-types'
import { BRIEF_SECTIONS, type BriefLine, type BriefSection } from './daily-brief-types'

const SECTION_TITLE: Record<BriefSection, string> = {
  action: 'Needs you',
  schedule: "Today's schedule",
  comms: 'Waiting on a reply',
  highlights: 'Worth knowing',
}

interface MirrorRow {
  briefDate: string
  artifactId: string | null
  readSeq: number
  agentName: string | null
  ownerEmail: string | null
  ownerName: string | null
}

/** Re-render the brief's artifact from the log. Creates it on first append. */
export async function mirrorBriefArtifact(briefId: string, userId: string): Promise<void> {
  const sql = await db()
  const rows = (await sql`
    select to_char(b.brief_date, 'YYYY-MM-DD') as "briefDate", b.artifact_id as "artifactId",
           b.read_seq as "readSeq", b.agent_name as "agentName",
           u.email as "ownerEmail", u.name as "ownerName"
    from daily_briefs b join users u on u.id = b.user_id
    where b.id = ${briefId} and b.user_id = ${userId}
  `) as unknown as MirrorRow[]
  const row = rows[0]
  if (!row) return

  const entries = (await sql`
    select id, seq, batch, kind, section, source_key as "sourceKey", source_type as "sourceType",
           source_id as "sourceId", source_href as "sourceHref", fingerprint, supersedes,
           priority, status_label as "statusLabel", badge, title, body, evidence,
           created_at as "createdAt"
    from daily_brief_entries where brief_id = ${briefId} order by seq asc
  `) as unknown as BriefEntry[]

  const title = `Daily brief — ${row.briefDate}`
  const actor = row.ownerEmail ?? row.ownerName ?? 'talaria'
  const body = renderBrief(entries, row)

  if (!row.artifactId) {
    // PRIVATE, and left that way. A brief is one person's attention state —
    // their unread DMs, their approvals, their blocked work — so the only
    // acceptable default is the one that discloses nothing. `visibility` is the
    // owner's to change from the artifact surface, deliberately, by hand.
    const created = await createArtifact({ kind: 'doc', title, createdBy: actor, ownerUserId: userId })
    await sql`update daily_briefs set artifact_id = ${created.id} where id = ${briefId} and artifact_id is null`
    // Re-read rather than trusting the insert: a concurrent append may have won
    // the race and created its own, in which case that one is the mirror and
    // this one is an orphan we simply stop writing to.
    const winner = (await sql`select artifact_id as "artifactId" from daily_briefs where id = ${briefId}`) as unknown as Array<{
      artifactId: string | null
    }>
    await saveArtifact(winner[0]?.artifactId ?? created.id, { body }, actor)
    return
  }
  await saveArtifact(row.artifactId, { title, body }, actor)
}

/** The log as markdown. Reads as the document does — lede, sections in document
 *  order, then the day's timeline — because a person who shares this is sharing
 *  what they were looking at, not a database dump. */
export function renderBrief(entries: BriefEntry[], row: Pick<MirrorRow, 'briefDate' | 'agentName' | 'readSeq'>): string {
  const { lines, updates } = foldEntries(entries, row.readSeq)
  const lede = entries.find((e) => e.kind === 'lede')
  const out: string[] = []

  if (lede?.body) out.push(lede.body, '')

  for (const section of BRIEF_SECTIONS) {
    const inSection = lines.filter((l) => l.section === section)
    if (inSection.length === 0) continue
    out.push(`## ${SECTION_TITLE[section]}`, '')
    for (const line of inSection) out.push(renderLine(line))
    out.push('')
  }

  if (updates.length > 0) {
    out.push('## How the day moved', '')
    // Oldest first here, unlike the surface. A shared document is read
    // top-to-bottom as a narrative; the live view leads with the newest because
    // its reader is checking what they missed.
    for (const update of [...updates].reverse()) {
      const at = new Date(update.at).toISOString().slice(11, 16)
      out.push(`**${at}** — ${update.note ?? `${update.entries.length} update(s)`}`)
      for (const entry of update.entries) {
        const verb = entry.kind === 'resolved' ? 'resolved' : entry.kind === 'change' ? 'changed' : 'new'
        out.push(`- _${verb}_ — ${entry.title}`)
      }
      out.push('')
    }
  }

  out.push('---', `_Written by ${row.agentName ?? 'your assistant'}. Appended to through the day; never rewritten._`)
  return out.join('\n')
}

function renderLine(line: BriefLine): string {
  const e = line.current
  const label = e.sourceHref ? `[${e.title}](${e.sourceHref})` : e.title
  const head = line.resolved ? `- ~~${label}~~` : `- **${label}**`
  const tags = [e.statusLabel, e.badge?.label].filter(Boolean).join(' · ')
  return [head, tags ? ` \`${tags}\`` : '', e.body && !line.resolved ? `\n  ${e.body}` : ''].join('')
}
