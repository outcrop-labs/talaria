// A SESSION, AS A DOCUMENT. The thread is the record of what you and the agent
// did, and until now it could only be read in the surface that rendered it:
// no way to hand it to somebody who does not use Talaria, no way to keep it
// beside the work it produced. This turns one conversation into markdown, which
// is what every other document on this platform already is — so the transcript
// becomes an artifact, and an artifact exports to Word, to Drive, to anywhere.
//
// PURE, AND TESTED, for the usual reason: the rules here are the ones that can
// be quietly wrong — a turn that swallows a message, an author label that
// attributes somebody's words to the agent, a key that survives into a file
// somebody is about to email.
//
// THE GRAMMAR IS THE RUST ONE. Run transcripts (talaria-runs-work-session,
// `capture_turn_transcript`) write `## Turn N` with `###` sub-sections under
// it, and a reader who has seen one of those should recognise this. Same
// heading shape; the sub-headings are WHO spoke rather than Prompt/Stream,
// because a conversation has two voices and a run has one.

import type { StoredMessage } from '@/lib/conversations.svelte'
import { cronJobFromMarker } from '@/components/chat/chat-view'

/** Credential shapes that must never leave in a transcript body — the mirror
 *  of `scrub_secrets` in talaria-runs-work-session, arm for arm. A tool result
 *  echoed into a reply can carry a raw key, and this body is about to be saved
 *  as a document, exported to Word, and mailed to somebody.
 *
 *  Kept as one source string on purpose (the Rust comment says why its own
 *  pattern is one physical line): an arm on its own line reads as separate
 *  until a stray escape silently disables it. */
const KEY_SHAPES = new RegExp(
  [
    'sk-(?:proj-)?[A-Za-z0-9_-]{16,}',
    'tak_[A-Za-z0-9_-]{8,}',
    'ghp_[A-Za-z0-9]{20,}',
    'github_pat_[A-Za-z0-9_]{20,}',
    'gsk_[A-Za-z0-9]{20,}',
    'xox[baprs]-[A-Za-z0-9-]{10,}',
    'AKIA[0-9A-Z]{16}',
  ].join('|'),
  'g',
)

/** Replace every credential shape with the Rust side's own marker, so a
 *  scrubbed transcript reads the same whichever half of the platform wrote it.
 *  Safe to call with the module-level regex: `String.replace` with a global
 *  pattern always starts at 0 and leaves `lastIndex` at 0. */
export function scrubSecrets(text: string): string {
  return text.replace(KEY_SHAPES, '[redacted:key]')
}

export interface TranscriptConversation {
  title: string | null
  /** When the thread started, if the caller holds it. The list read does not
   *  carry one, so the line is omitted rather than guessed. */
  createdAt?: string
}

export interface TranscriptOptions {
  /** What the agent's turns are labelled. The harness's own `agentLabel`,
   *  which outlives the agent that spoke (a run's recorded name). */
  agentLabel?: string
  /** Name the tools each reply ran. Off by default: a transcript is a record
   *  of the conversation first, and the tool line is for somebody auditing it. */
  includeTools?: boolean
  /** Include the model's reasoning where a row carries it. Off by default —
   *  it is long, and it is not what was SAID. */
  includeReasoning?: boolean
}

/** YYYY-MM-DD in UTC. UTC rather than the reader's clock so one session gets
 *  one name wherever it is exported from: two people exporting the same
 *  transcript either side of midnight should not produce two documents. */
const day = (d: Date): string => d.toISOString().slice(0, 10)

/** The artifact name for a session's transcript. */
export function transcriptTitle(conversation: { title: string | null }, now: Date = new Date()): string {
  const stem = conversation.title?.trim() || 'Session'
  return `${stem} — transcript ${day(now)}`
}

/** The stamps a row wears, as italics under the author line. Each one answers
 *  a question the bare text raises: why a message sits inside somebody else's
 *  turn, why a reply stops mid-sentence, why the agent spoke with nobody in
 *  the room. */
function stampLines(m: StoredMessage): string[] {
  const meta = m.metadata ?? {}
  const out: string[] = []
  if (meta.steer === true) out.push('_Said into the reply that was already streaming._')
  if (meta.stopped === true) out.push('_Stopped before it finished._')
  if (meta.resumed === true) out.push('_Resumed after the stream died mid-turn._')
  const cron = cronJobFromMarker(meta.marker)
  if (cron) out.push(`_Posted by a scheduled run (job \`${cron}\`)._`)
  if (m.status === 'error') out.push('_This turn errored._')
  return out
}

/** Tool names, once each, in the order the reply ran them. Identifiers, so
 *  they keep the mono voice markdown gives a backtick. */
function toolLine(m: StoredMessage): string | null {
  const names: string[] = []
  for (const t of m.tools ?? []) {
    const name = t.name?.trim()
    if (name && !names.includes(name)) names.push(name)
  }
  if (names.length === 0) return null
  return `Tools: ${names.map((n) => `\`${n}\``).join(', ')}`
}

function attachmentLines(m: StoredMessage): string[] {
  const files = (m.attachments ?? []).map((a) => a.filename?.trim()).filter((f): f is string => !!f)
  if (files.length === 0) return []
  return ['Attachments:', '', ...files.map((f) => `- ${f}`)]
}

/** A message's own ATX headings are escaped so what somebody wrote can never
 *  read as the transcript's structure: a line "### Ada" inside a message is
 *  text, not a speaker, and "## Turn 9" is not a turn. */
export const escapeHeadings = (s: string): string => s.replace(/^(\s{0,3})(#{1,6}\s)/gm, '$1\\$2')

/** One row as its own block: who spoke, what the row is stamped with, what was
 *  said, and what travelled with it. */
function rowBlock(m: StoredMessage, heading: string, opts: TranscriptOptions): string[] {
  const out: string[] = [`### ${heading}`, '']
  for (const stamp of stampLines(m)) out.push(stamp, '')
  const body = escapeHeadings(scrubSecrets(m.content ?? '').trim())
  out.push(body || '_No text in this turn._', '')
  if (opts.includeReasoning) {
    const reasoning = escapeHeadings(scrubSecrets(m.reasoning ?? '').trim())
    if (reasoning) {
      out.push('_Reasoning_', '')
      // A blockquote, so the reasoning cannot be mistaken for what was said.
      for (const line of reasoning.split('\n')) out.push(`> ${line}`)
      out.push('')
    }
  }
  if (opts.includeTools) {
    const tools = toolLine(m)
    if (tools) out.push(tools, '')
  }
  const files = attachmentLines(m)
  if (files.length > 0) out.push(...files, '')
  return out
}

/**
 * A conversation as markdown.
 *
 * TURNS. A new `## Turn N` opens on every user row, and on an assistant row
 * that has no turn open yet (a research run's own posts and a scheduled run's
 * output arrive with nobody having asked in the thread). Every row lands in
 * exactly one turn and no row is ever dropped — a transcript that silently
 * loses a message is worse than one whose numbering reads oddly, which is why
 * grouping is deliberately this simple and not an attempt to guess which reply
 * answered which queued message.
 */
export function transcriptToMarkdown(
  conversation: TranscriptConversation,
  messages: StoredMessage[],
  opts: TranscriptOptions = {},
): string {
  const agent = opts.agentLabel?.trim() || 'Agent'
  const lines: string[] = [`# ${conversation.title?.trim() || 'Session'}`, '']
  if (conversation.createdAt) lines.push(`_Started ${conversation.createdAt}_`, '')

  let turn = 0
  let open = false
  for (const m of messages) {
    if (m.role === 'user' || !open) {
      turn += 1
      open = true
      lines.push(`## Turn ${turn}`, '')
    }
    const heading = m.role === 'user' ? m.authorLabel?.trim() || 'You' : agent
    lines.push(...rowBlock(m, heading, opts))
  }
  if (turn === 0) lines.push('_Nothing was said in this session._', '')
  // One trailing newline: a markdown file ends in exactly one.
  return `${lines.join('\n').trimEnd()}\n`
}
