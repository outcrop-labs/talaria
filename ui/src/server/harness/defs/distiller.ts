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

/** ONE DECISION AND NOTHING ELSE. The easy floor: a model that cannot carry a
 *  single stated decision through a four-line chat cannot do this job at all. */
const ONE_DECISION = [
  'User: quick one — do we cap the free tier at 3 seats or 5?',
  'Nomad: what were we leaning towards?',
  'User: three. Locked, do not reopen it.',
  'Nomad: noted.',
].join('\n\n')

/** A chat where the substance is a decision NOT to do something. The failure to
 *  catch is a distillation that records the idea and loses the rejection, which
 *  is how a dropped plan comes back next quarter as an agreed one. */
const NEGATIVE_DECISION = [
  'User: should we build the Zendesk importer this quarter?',
  'Nomad: it came up in planning twice.',
  'User: no. We are explicitly not doing Zendesk this quarter — the effort goes to the ledger migration instead.',
  'Nomad: understood, ledger migration takes the slot.',
].join('\n\n')

/** A chat that CHANGES ITS MIND. Only the last position is durable, and a model
 *  that flattens the conversation records both and contradicts itself. */
const REVERSAL = [
  'User: let us put the rate limiter in the gateway.',
  'Nomad: that would cover every caller at once.',
  'User: actually no — scratch that. The gateway cannot see per-tenant quota, so it goes in the API layer where the tenant is known.',
  'Nomad: API layer it is.',
  'User: right, and Ivan owns it.',
].join('\n\n')

/** ALL CHATTER, NO SUBSTANCE. The right distillation says so; the failure is
 *  inventing a decision to have something to write down. */
const NOTHING_DURABLE = [
  'User: morning!',
  'Nomad: morning — anything you need?',
  'User: no, just checking you were up. Have a good one.',
  'Nomad: you too.',
].join('\n\n')

/** A chat carrying a NUMBER and a DATE, which are the two things a paraphrase
 *  quietly rounds away. */
const NUMBERS = [
  'User: what did we settle on for the retry budget?',
  'Nomad: nothing written down yet.',
  'User: five attempts, exponential backoff, give up after 30 seconds total. Starts in the 4.2 release on 12 March.',
  'Nomad: five attempts, 30 second ceiling, 4.2 on the 12th.',
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
  // NINE FIXTURES, THREE BANDS. The two this file shipped with both run on the
  // same transcript, so a model that happened to handle THAT conversation
  // scored 100% on the distiller. The bands below vary the SHAPE of the
  // conversation — one decision, a rejection, a reversal, numbers, nothing at
  // all — because those are the shapes that break a small model differently.
  evals: [
    {
      name: 'carries a single stated decision',
      band: 'easy',
      input: { agentLabel: 'Nomad', transcript: ONE_DECISION },
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 20, mentions: ['three', '3', 'seat', 'free tier'] })
        if (thin) return thin
        return value.length < ONE_DECISION.length ? null : 'the distillation is no shorter than the transcript'
      },
    },
    {
      name: 'keeps a number and a date exactly as they were stated',
      band: 'easy',
      input: { agentLabel: 'Nomad', transcript: NUMBERS },
      check: (value) => {
        const v = value.toLowerCase()
        const lost = [
          { term: 'five attempts', ok: /\bfive\b|\b5\b/.test(v) },
          { term: 'the 30 second ceiling', ok: /\b30\b|thirty/.test(v) },
          { term: 'the 4.2 release', ok: /4\.2/.test(v) },
        ].filter((x) => !x.ok)
        return lost.length ? `paraphrased away ${lost.map((x) => x.term).join(', ')}` : null
      },
    },
    {
      name: 'keeps the planted decisions and drops the planted pleasantries',
      band: 'standard',
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
      band: 'standard',
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
    {
      name: 'records a decision NOT to do something as a decision',
      band: 'standard',
      // A distillation that keeps "Zendesk importer" and loses "no" is how a
      // rejected plan comes back next quarter as an agreed one.
      input: { agentLabel: 'Nomad', transcript: NEGATIVE_DECISION },
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 30, mentions: ['zendesk', 'ledger'] })
        if (thin) return thin
        const v = value.toLowerCase()
        const refused = /\bnot\b|\bno\b|declin|reject|drop|defer|skip|instead/.test(v)
        return refused ? null : 'recorded the Zendesk importer without recording that it was explicitly turned down'
      },
    },
    {
      name: 'skips process chatter in a conversation that is mostly process chatter',
      band: 'standard',
      input: {
        agentLabel: 'Nomad',
        transcript: [
          'User: sorry for the slow reply, back to back all morning',
          'Nomad: no problem at all.',
          'User: anyway — the API version header is going to be required from 1 June. Unversioned callers get a 400 after that.',
          'Nomad: got it.',
          'User: thanks, sorry again for the delay, talk later!',
        ].join('\n\n'),
      },
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 25, mentions: ['version', 'header', 'june', '400'] })
        if (thin) return thin
        const chatter = ['sorry', 'slow reply', 'talk later', 'back to back'].filter((k) => value.toLowerCase().includes(k))
        return chatter.length ? `kept process chatter it was told to skip: ${chatter.join(', ')}` : null
      },
    },
    // ── hard ────────────────────────────────────────────────────────────────
    {
      name: 'keeps only the position the conversation ended on',
      band: 'hard',
      // The reversal. A model that flattens the chat records both placements and
      // leaves the brain holding a contradiction.
      input: { agentLabel: 'Nomad', transcript: REVERSAL },
      check: (value) => {
        const thin = belowAnswerFloor(value, { minChars: 30, mentions: ['api', 'rate limit', 'ivan'] })
        if (thin) return thin
        const v = value.toLowerCase()
        if (!/api/.test(v)) return 'lost the decision the conversation actually landed on (the API layer)'
        // Naming the gateway is fine as the rejected option; presenting it as
        // the decision is not.
        const presentsGateway = /(?:goes|lives|sits|put|place)\w*\s+(?:it\s+)?in\s+the\s+gateway/.test(v)
        return presentsGateway ? 'recorded the reversed decision (the gateway) as if it still stood' : null
      },
    },
    {
      name: 'says a conversation held nothing durable rather than inventing something',
      band: 'hard',
      // "Never invent anything" is the prompt's hardest rule to keep, because
      // an empty answer feels like a failure to a model. It is the right one.
      input: { agentLabel: 'Nomad', transcript: NOTHING_DURABLE },
      check: (value) => {
        const v = value.trim().toLowerCase()
        if (v.length === 0) return 'returned nothing at all — the contract asks for a line saying there was nothing durable'
        const saysNothing = /nothing|no decision|no durable|none|small talk|pleasantr|greeting|nothing was decided|no action/.test(v)
        if (saysNothing) return null
        return `invented substance for a conversation that had none: "${value.trim().slice(0, 90)}"`
      },
    },
  ],
})
