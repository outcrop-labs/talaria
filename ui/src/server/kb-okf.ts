// OKF digests — each knowledge space's agent-facing summary, maintained by
// the LIBRARIAN platform agent. An OKF is DERIVED knowledge: the summation of
// a space's PROMOTED (official) documents — a summary and link per doc —
// regenerated autonomously whenever promotions change. It is itself official,
// so it grounds agents through the org brain; nobody writes it by hand.
import { db } from './db/pg'
import { completeViaGateway, gatewayModels, resolveRoute } from './llm-gateway'
import { resolveRoleModel } from './model-roles'
import { platformAgentModel } from './platform-agents'
import { createDoc, getSpace, saveDoc, setOfficial, type KbDoc } from './kb'

const clip = (s: string, max = 2400) => (s.length > max ? `${s.slice(0, max)}\n…(truncated)` : s)

async function librarianModel(): Promise<string | null> {
  const pinned = await platformAgentModel('librarian')
  if (pinned) return pinned
  const utility = await resolveRoleModel('utility')
  if (utility) return utility
  for (const m of [process.env.TALARIA_COPILOT_MODEL ?? null, 'pl-main']) {
    if (m && (await resolveRoute(m))) return m
  }
  return (await gatewayModels()).find((m) => !m.qualified)?.id ?? null
}

const PROMPT =
  'You are the librarian maintaining an ORGANIZATION KNOWLEDGE DIGEST for AI agents. ' +
  'From the documents provided, write a digest: a one-paragraph overview of what this space covers, then one "## <title>" section per document ' +
  'containing a crisp 2-4 sentence summary of its substance followed by the provided link line verbatim. ' +
  'Factual, terse, no invention, no commentary about the digest itself. Reply with ONLY the digest markdown.'

/** Regenerate one space's OKF digest from its promoted docs. */
export async function regenerateOkf(spaceId: string): Promise<void> {
  const sql = await db()
  const space = await getSpace(spaceId)
  if (!space) return
  const model = await librarianModel()
  if (!model) return

  const docs = (await sql`
    select id, title, body from kb_docs
    where space_id = ${spaceId} and official and visibility <> 'private'
      and (${space.okfDocId ?? null}::uuid is null or id <> ${space.okfDocId ?? null})
    order by updated_at desc limit 14
  `) as unknown as Array<{ id: string; title: string; body: string }>

  let body: string
  if (docs.length === 0) {
    body = `_No promoted documents yet. Promote documents in this space and the Librarian will digest them here._`
  } else {
    const source = docs
      .map((d) => `### ${d.title}\nLink: [Open the full document](/knowledge?d=${d.id})\n\n${clip(d.body)}`)
      .join('\n\n---\n\n')
    const { text } = await completeViaGateway(
      model,
      [
        { role: 'system', content: PROMPT },
        { role: 'user', content: `Space: "${space.name}"${space.body ? ` — ${clip(space.body, 600)}` : ''}\n\nDocuments:\n\n${source}` },
      ],
      { temperature: 0.2, caller: 'platform:librarian' },
    )
    if (!text.trim()) return // model hiccup — keep the previous digest
    body = `> Auto-generated digest of promoted documents in **${space.name}** — maintained by the Librarian. Edits here are overwritten.\n\n${text.trim()}`
  }

  // Upsert the digest doc (kind 'agent' now MEANS generated OKF).
  let doc: KbDoc | null = null
  if (space.okfDocId) {
    doc = await saveDoc(space.okfDocId, { title: `${space.name} — OKF digest`, body }, 'librarian')
  }
  if (!doc) {
    doc = await createDoc({ spaceId, title: `${space.name} — OKF digest`, kind: 'agent', createdBy: 'librarian', ownerUserId: null })
    await saveDoc(doc.id, { body }, 'librarian')
    await sql`update kb_spaces set okf_doc_id = ${doc.id} where id = ${spaceId}`
  }
  // Official by definition — the digest is what grounds agents.
  if (docs.length > 0) await setOfficial(doc.id, true, 'librarian')
}

// ── Debounced autonomy: promotions queue their space; bursts collapse. ──────
const timers = new Map<string, ReturnType<typeof setTimeout>>()
export function queueOkfRegen(spaceId: string): void {
  const prior = timers.get(spaceId)
  if (prior) clearTimeout(prior)
  timers.set(
    spaceId,
    setTimeout(() => {
      timers.delete(spaceId)
      void regenerateOkf(spaceId).catch(() => {})
    }, 20_000),
  )
}
