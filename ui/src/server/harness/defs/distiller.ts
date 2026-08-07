// The Distiller: what survives an idle agent DM after the scrollback goes away.
//
// This harness is load-bearing in a way none of the other leaf harnesses are.
// A titler that fails leaves a chat with a boring name. This one runs on the
// LAST PASS a conversation ever gets: `comms-decay.ts` distills, indexes into
// the owner's private brain, and then archives the chat out of the sidebar. If
// the distillation is empty or wrong, the substance of that conversation is
// gone — which is exactly why `comms-decay.ts` refuses to archive on a failed
// distillation, and why `onFailure` here is 'null' rather than a fallback
// string. There is no safe placeholder for "we lost it": the only correct
// answer to a failed distillation is to leave the conversation alone and let
// the next sweep try again.
//
// PORTED FROM comms-decay.ts (audit 1.10). The prompt, the temperature and the
// user turn are the originals, unchanged. What went away was the hand-copied
// model chain and the bare `if (!text.trim())` — the runner owns both now.
import { belowAnswerFloor, defineHarness } from '../define'

export interface DistillInput {
  /** How the agent is named in the transcript. The distillation is read back
   *  by the chat's owner, so it uses the label they saw, never a model id. */
  agentLabel: string
  /** The rendered conversation, already clipped by the caller. Clipping stays
   *  with the caller because it is the thing that knows the message rows. */
  transcript: string
}

/** The original prompt, preserved verbatim. Three clauses do the real work and
 *  none of them are decoration:
 *    "Skip pleasantries"  — the eval fixture below plants two and asserts they
 *                           do not survive.
 *    "Never invent"       — this text is indexed into a private brain and later
 *                           retrieved as fact by the owner's assistant. An
 *                           invented decision here becomes a remembered one.
 *    "ONLY the distillation" — a preamble ("Here is the distillation:") is
 *                           stored as if it were substance; nothing downstream
 *                           strips it. */
const NARROW =
  'Distill this conversation into its durable substance: decisions made, facts established, preferences expressed, and outcomes — terse markdown bullets, grouped when helpful. ' +
  'Skip pleasantries and process chatter. Never invent anything. Reply with ONLY the distillation.'

/** The widened prompt. Same job, structured — because retrieval reads this
 *  text, and a distillation whose decisions sit under a heading is a
 *  distillation whose decisions can be found again.
 *
 *  The heading list is NOT what makes this need a capable model. The
 *  omit-rather-than-pad rule is. Hand a small model five headings and it fills
 *  all five, because an empty section reads to it like an unfinished answer —
 *  and a padded "Preferences" section under a conversation that expressed none
 *  is invention, filed as memory, in the one harness that must never invent.
 *  So the structure is gated on the model being KNOWN to hold an instruction
 *  that says "write less". */
const WIDE = [
  'Distill this conversation into its durable substance, under these headings and in this order:',
  '## Decisions  ## Facts  ## Preferences  ## Outcomes  ## Open',
  '',
  'Terse markdown bullets under each heading. Use ONLY the headings this conversation actually earns:',
  'omit a heading entirely rather than writing a bullet you could not point at a specific line of the',
  'transcript for. An empty section is the correct answer when nothing was decided.',
  'Skip pleasantries and process chatter. Never invent anything. Reply with ONLY the distillation.',
].join('\n')

/** A conversation that plants three decisions among pleasantries. The check is
 *  a substring test on purpose (audit Part 3): "Postgres", "Friday" and
 *  "Nadia" are the load-bearing tokens of the three decisions, and no faithful
 *  distillation of this transcript can omit them or paraphrase them away. */
const FIXTURE = [
  'User: Morning! Hope you had a good weekend.',
  'Nomad: Thanks, all good. What are we doing about the ledger store?',
  'User: We are going with Postgres over SQLite. That is locked.',
  'Nomad: Understood. Timing?',
  'User: Ship the ledger migration on Friday.',
  'Nomad: Who runs the rollback if it goes wrong?',
  'User: Nadia owns the rollback plan.',
  'Nomad: Got it. Enjoy the rest of your day!',
].join('\n\n')

export const distillerHarness = defineHarness<DistillInput, string>({
  id: 'distiller',
  label: 'Distiller',
  job: 'Condenses idle agent chats into their durable substance before they archive.',
  // Not 'long-context': the caller clips the transcript to 60k characters, so
  // the ask is bounded. What it genuinely needs is a model that honors "skip
  // this, never invent that, reply with only the answer" — three constraints in
  // one system turn, which is precisely the instruction-following probe.
  requires: ['instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    // Nothing is refusable here. A thin distillation is a worse memory; NO
    // distillation is a conversation that never decays, because comms-decay
    // will not archive what it could not summarize. Degrading is the product
    // working as designed.
    note: 'A smaller model writes a thinner distillation and may miss a decision, but the chat still archives with something in your brain rather than nothing.',
  },
  // The chain comms-decay hand-wrote: the Distiller's assigned model, else the
  // owner's own muse. `userId` on the run context turns on the 'preferred' step
  // and the member model allowlist, which is what `museModelFor` did by hand.
  model: { pin: 'distiller', chain: ['pin', 'preferred', 'utility', 'env', 'first-routable'] },
  render: (input, ctx) => [
    { role: 'system', content: ctx.widened ? WIDE : NARROW },
    { role: 'user', content: `Conversation with ${input.agentLabel}:\n\n${input.transcript}` },
  ],
  // Trim, and treat whitespace as nothing. `comms-decay.ts` used to spell this
  // as `if (!text.trim()) return 'empty-distillation'`; the contract is the
  // same one, stated where every other harness states it.
  output: { kind: 'text', clean: (raw) => raw.trim() || null },
  // The caller keeps what it had, and what it had is an unarchived conversation
  // with all of its messages. Any other policy — a fallback string especially —
  // would let the sweep archive a chat whose substance was never captured.
  onFailure: 'null',
  widen: {
    requires: ['instruction-following'],
    note: 'Models proven to follow a "write less" instruction get a sectioned distillation instead of a flat bullet list.',
  },
  // NARROWED DELIBERATELY. A distillation is a RECORD of what an agent said it
  // did, not a fresh claim, so `zero_tool_claim` and `fabricated_outage` fire
  // on faithful summaries of any conversation where work happened or something
  // broke. Leaving them on would fill `guard_findings` — the live per-model
  // confabulation rate the fitness page reads — with the distiller doing its
  // job correctly. What is worth catching is a credential the owner pasted into
  // the chat: the scrollback is about to be archived away, so this text is
  // where it would OUTLIVE the conversation, in a brain the assistant retrieves
  // from. That is why `redact` is on here and not merely observed.
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  temperature: 0.2,
  evals: [
    {
      name: 'keeps the planted decisions and drops the planted pleasantries',
      input: { agentLabel: 'Nomad', transcript: FIXTURE },
      check: (value) => {
        const v = value.toLowerCase()
        const dropped = ['postgres', 'friday', 'nadia'].filter((k) => !v.includes(k))
        if (dropped.length) return `lost planted decision(s): ${dropped.join(', ')}`
        const kept = ['weekend', 'enjoy'].filter((k) => v.includes(k))
        if (kept.length) return `kept pleasantries it was told to skip: ${kept.join(', ')}`
        return null
      },
    },
    {
      name: 'is shorter than the conversation it distills',
      input: { agentLabel: 'Nomad', transcript: FIXTURE },
      // A "distillation" longer than its source is a model restating the
      // transcript, which is the small-model failure this job actually hits —
      // and it passes every content assertion above while being useless.
      //
      // The FLOOR is the other half, and it was missing: this assertion is a
      // pure upper bound, so a two-word non-answer satisfied it perfectly. A
      // distillation of a conversation whose whole substance is three decisions
      // has to be at least a sentence, and has to have engaged with one of
      // them.
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 40, mentions: ['postgres', 'ledger', 'friday', 'nadia', 'rollback'] })
        if (thin) return thin
        return value.length < FIXTURE.length ? null : 'the distillation is no shorter than the transcript - the model restated it rather than distilling it'
      },
    },
  ],
})
