// Distill-then-archive: conversations with agents don't accumulate forever.
// Idle agent DMs get their durable substance summarized into the activity
// brain (owner-scoped), then archive out of the sidebar — context survives,
// scrollback doesn't. Relays conclude explicitly: summary posted + indexed,
// then the relay archives. Everything here is fire-and-forget friendly.
import { db } from './db/pg'
import { agentCategoryFolder, createArtifact, saveArtifact } from './artifacts'
import { archiveChannel, insertChannelMessage, listChannelAgents, listChannelMessages } from './channels'
import { describeAgent } from './gateway'
import { concluderHarness } from './harness/defs/concluder'
import { distillerHarness } from './harness/defs/distiller'
import { runHarness } from './harness/run'
import { indexActivity, indexPersonal } from './retrieval/sources'
import { registerJob } from './scheduler'

const TTL_DAYS = () => Math.max(1, Number(process.env.TALARIA_CHAT_TTL_DAYS ?? 14))
// The deploy-day bound. This sweep ARCHIVES people's conversations, and until
// now it only ran when traffic happened to kick it — so the first instance to
// run it on a schedule may find a long backlog. One pass never touches more
// than this many, and a pass runs at most hourly, so the worst case is ~8
// archives an hour (~190/day) draining gradually rather than a wall of
// "where did my chats go" on the morning of a deploy.
const SWEEP_BATCH = 8

const clip = (s: string, max = 60_000) => (s.length > max ? `${s.slice(0, max)}\n…(truncated)` : s)

/** What one conversation's turn actually did. The sweep counts these rather
 *  than assuming, because two of the three are ways this function returns
 *  having archived NOTHING, and both used to be counted as an archive.
 *
 *  A job that reports work it did not do is worse than a job that reports a
 *  failure: the log line is green, /observability is quiet, and the only symptom
 *  is chats that never decay on an instance whose operator has been reading
 *  "8 idle chat(s) distilled + archived" every hour for a month. */
type DistillOutcome =
  /** Distilled (or empty and nothing to distill), indexed, and archived. */
  | 'archived'
  /** No Distiller model and no muse for the owner — nothing can summarize it,
   *  so it is left exactly where it was for a sweep that has one. */
  | 'no-model'
  /** The model answered with nothing. Left unarchived on purpose: archiving on
   *  a failed distillation is how the substance is lost. */
  | 'empty-distillation'

/** What a harness run means to this file: a usable distillation, or the
 *  non-archiving outcome it has to be counted as.
 *
 *  THE TWO NULLS ARE NOT THE SAME EVENT, and this function exists so nothing
 *  ever has to re-derive which is which. `runHarness` reports `model` and
 *  `value` separately for exactly this reason:
 *
 *    model === null   nothing is CONFIGURED to summarize with. Every
 *                     conversation in the batch will hit it, it will still be
 *                     true in an hour, and the sweep escalates it to a human
 *                     because only a human can assign a model.
 *    value === null   a model was asked and could not answer. This one
 *                     conversation is left alone and retried next pass.
 *
 *  Collapsing them loses the escalation. Treating either as success archives a
 *  conversation whose substance was never captured — the exact failure this
 *  whole file is written around. Exported so the mapping is tested directly.
 */
export function distillOutcome<T>(run: { model: string | null; value: T | null }): { ok: true; value: T } | { ok: false; outcome: Exclude<DistillOutcome, 'archived'> } {
  if (!run.model) return { ok: false, outcome: 'no-model' } // no routable model — leave it for a sweep that has one
  if (run.value === null) return { ok: false, outcome: 'empty-distillation' } // don't archive on a failed distillation
  return { ok: true, value: run.value }
}

/** Distill one idle agent DM into the activity brain, then archive it. */
async function distillConversation(conv: {
  id: string
  userId: string
  agentModel: string
  title: string | null
}): Promise<DistillOutcome> {
  const sql = await db()
  const msgs = (await sql`
    select role, content from messages
    where conversation_id = ${conv.id} and content <> '' order by seq asc
  `) as unknown as Array<{ role: string; content: string }>
  const label = describeAgent(conv.agentModel).label
  const transcript = clip(msgs.map((m) => `${m.role === 'assistant' ? label : 'User'}: ${m.content}`).join('\n\n'))

  // BEHAVIOR CHANGE, deliberate: a conversation with nothing in it now archives
  // without needing a model at all. The chain used to be resolved first, so a
  // wholly empty chat on an install with no Distiller model came back
  // 'no-model' — unarchivable for ever, and counted into the skippedNoModel
  // total that makes the job throw. There is no substance to lose here, so the
  // "never archive on a failed distillation" rule has nothing to protect.
  if (transcript.trim()) {
    // The model chain, the empty-reply check and the failure policy all moved
    // into harness/defs/distiller.ts. `userId` is what turns on the owner's
    // preferred model and the member allowlist — the two things `museModelFor`
    // used to supply by hand.
    const run = await runHarness(distillerHarness, { agentLabel: label, transcript }, { caller: `platform:distiller:${conv.userId}`, userId: conv.userId })
    const read = distillOutcome(run)
    if (!read.ok) return read.outcome
    const text = read.value
    const title = `Distilled: ${conv.title || `chat with ${label}`}`
    // Twice on purpose: the activity copy keeps the owner's ambient search
    // working as before (owner-scoped), and the personal-brain copy is what
    // their assistant retrieves — its private memory of this user's history.
    // Search merges dedupe by source, so the owner never sees it doubled.
    const distillDoc = {
      sourceType: 'chat-distill',
      sourceId: conv.id,
      title,
      text,
      payload: { ownerUserId: conv.userId },
      href: '/comms',
    }
    await indexActivity(distillDoc)
    await indexPersonal(conv.userId, distillDoc)
    // The distill is also a browsable artifact — PRIVATE to the chat's owner
    // (a DM's substance is theirs), filed under the agent's "Chat summaries".
    try {
      const artifact = await createArtifact({
        kind: 'doc',
        title,
        createdBy: label,
        ownerUserId: conv.userId,
        folderId: await agentCategoryFolder(label, 'Chat summaries', label),
      })
      await saveArtifact(artifact.id, { body: text }, label)
    } catch {
      /* filing is best-effort — the distillation is already indexed */
    }
  }
  await sql`update conversations set archived = true where id = ${conv.id}`
  return 'archived'
}

export interface DecaySweepResult {
  /** Idle chats this pass picked up. `archived + skippedNoModel +
   *  skippedEmptyDistillation + failed` always equals it — the sweep accounts
   *  for every conversation it touched, which is the property that makes the log
   *  line checkable. */
  considered: number
  archived: number
  /** Left alone because nothing is configured to summarize them. NOT a per-
   *  conversation fault: it is the Distiller platform agent having no model and
   *  the owner having no muse, and it will be true again on every pass until
   *  somebody assigns one. */
  skippedNoModel: number
  /** The model was asked and answered with nothing. Retried next pass. */
  skippedEmptyDistillation: number
  failed: number
}

/** One pass: distill + archive up to SWEEP_BATCH idle agent DMs. Plans are
 *  exempt — they're durable documents, not chat scrollback. */
export async function sweepIdleChats(): Promise<DecaySweepResult> {
  const sql = await db()
  const idle = (await sql`
    select id, user_id as "userId", agent_model as "agentModel", title
    from conversations
    where kind = 'chat' and archived = false
      and updated_at < now() - make_interval(days => ${TTL_DAYS()})
    order by updated_at asc
    limit ${SWEEP_BATCH}
  `) as unknown as Array<{ id: string; userId: string; agentModel: string; title: string | null }>
  const result: DecaySweepResult = {
    considered: idle.length,
    archived: 0,
    skippedNoModel: 0,
    skippedEmptyDistillation: 0,
    failed: 0,
  }
  for (const conv of idle) {
    try {
      // Counted by what came back, never by "it did not throw". The version this
      // replaced incremented `archived` on every non-throwing call, so the two
      // early returns inside `distillConversation` — no model, empty summary —
      // were both reported as archives that had not happened.
      const outcome = await distillConversation(conv)
      if (outcome === 'archived') result.archived++
      else if (outcome === 'no-model') result.skippedNoModel++
      else result.skippedEmptyDistillation++
    } catch (e) {
      // One bad conversation must not abandon the rest of the batch — but a
      // conversation that fails every pass forever used to be invisible, so
      // name it. The next sweep retries it.
      result.failed++
      console.error(`[comms-decay] conversation ${conv.id} could not be distilled:`, e instanceof Error ? e.message : e)
    }
  }
  return result
}

// Scheduled, not kicked. This used to be `maybeSweepIdleChats()` called from
// GET /api/channels with an hourly module-level timestamp, which meant an
// instance nobody was reading comms on never decayed anything at all.
registerJob({
  name: 'comms-decay',
  everyMs: 60 * 60_000, // unchanged: the throttle the kick already enforced
  // Not at the instant of boot: a crash-looping instance should never reach a
  // job that archives, and a deploy should settle before it starts writing.
  firstRunDelayMs: 2 * 60_000,
  // SWEEP_BATCH distillations, each an LLM round trip; generous, and the lease
  // renews while it runs anyway.
  maxRunMs: 15 * 60_000,
  run: async () => {
    const r = await sweepIdleChats()
    if (!r.considered) return null
    const line =
      `${r.archived} idle chat(s) distilled + archived` +
      (r.skippedEmptyDistillation ? `, ${r.skippedEmptyDistillation} left alone (the summary came back empty)` : '') +
      (r.failed ? `, ${r.failed} failed` : '')
    // NOTHING TO SUMMARIZE WITH IS A FAILED RUN, NOT A QUIET ONE. The sweep
    // picked these conversations up, could not act on a single one of them, and
    // will pick the same ones up again in an hour and every hour after that —
    // for ever, silently, because a `return` inside the distiller used to be
    // counted as an archive. Thrown so it lands in the scheduler's error state,
    // which is what `unhealthyJobs()` reads and what puts it on /observability
    // in front of a person who can assign a model.
    if (r.skippedNoModel) {
      throw new Error(
        `${r.skippedNoModel} of ${r.considered} idle chat(s) could not be distilled: no model is assigned to the` +
          ' Distiller platform agent (Admin → Platform agents) and their owners have no muse model either, so nothing' +
          ` can summarize them and they will not decay. ${line}.`,
      )
    }
    return line
  },
})

/** Conclude a Relay: post + index a summary of what was decided, then archive.
 *  Returns the summary so the UI can show it after the relay leaves the list. */
export async function concludeRelay(channelId: string, byUserId: string, channelName: string): Promise<string> {
  const history = await listChannelMessages(channelId, -1, 500, { includeThreads: true })
  const transcript = clip(
    history
      .filter((m) => m.status === 'complete' && m.content)
      .map((m) => `${m.authorType === 'agent' ? describeAgent(m.author).label : m.author}: ${m.content}`)
      .join('\n\n'),
  )
  if (!transcript.trim()) throw new Error('nothing to conclude: the relay has no messages')

  // The model chain and the empty-reply check moved into
  // harness/defs/concluder.ts. The failures are still mapped BY HAND rather than
  // with `onFailure: 'throw'`, and the reason is down to one: these strings are
  // USER-FACING COPY shown by the conclude button, not a developer's error
  // message. ('throw' would now cover all three — it stopped being contract-only
  // — so the other half of this note is gone: the runner no longer returns for
  // an unresolved chain under that policy.)
  const run = await runHarness(concluderHarness, { channelName, transcript }, { caller: `platform:concluder:${byUserId}`, userId: byUserId })
  if (!run.model) throw new Error('no model configured to summarize with. Add an endpoint on /models.')
  // THREE outcomes, not two. `runHarness` also returns for a transport failure,
  // and folding that into "came back empty" told a user whose provider was rate
  // limiting to try again — into the same rate limit — while the only sentence
  // that explains it (`gateway completion 429: …`, which is what the button
  // showed before the port) sat on the `harness_runs` row. The runner's sentence
  // is the right one whenever the model never answered, and `answered` is that
  // question under its own name: this asked `raw === null` before, which is a
  // drill-down field pressed into service as a control-flow test and which reads
  // a stream that died after three tokens as a model that answered.
  if (!run.answered && run.error) throw new Error(run.error)
  if (run.value === null) throw new Error('the summary came back empty. Try again.')
  const text = run.value

  // The summary is the relay's last word: posted into history (visible if the
  // relay is ever revisited) and indexed for retrieval (channel-membership ACL).
  const agents = await listChannelAgents(channelId)
  await insertChannelMessage(channelId, 'agent', agents[0] ?? 'talaria', `**Relay concluded**. Summary:\n\n${text}`)
  await indexActivity({
    sourceType: 'relay-summary',
    sourceId: channelId,
    title: `Relay concluded: ${channelName}`,
    text,
    payload: { channelId },
    href: '/comms',
  })
  await archiveChannel(channelId)
  return text
}
