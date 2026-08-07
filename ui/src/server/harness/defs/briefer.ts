// The Briefer's two harnesses: the per-view briefing, and the chat-back on it.
//
// WHY THIS FILE EXISTS (audit 1.5)
//   Both halves reached a model through `proxyChat` and were therefore in the
//   "no guardrail at all" column: `server/briefing.ts` built its prompt inline,
//   drained the persona stream by hand, hand-rolled the empty-reply check and
//   metered itself. The briefing is then PERSISTED to `briefings.summary` and
//   re-read on every poll, so a credential quoted out of a notification title
//   or a ticket subject was stored verbatim and re-rendered forever, and
//   nothing anywhere noticed. The chat-back was likewise unguarded, while the
//   very same question asked in ordinary chat goes through `guardChatReply`.
//
// THE MODEL IS FIXED, AND IT IS THE ONLY HARNESS IN THE PRODUCT THAT IS.
//   `PLATFORM_AGENTS.briefer` is the one entry with `assignable: false`, and
//   its `auto` line says why in the product's own words: "always the user's
//   personal assistant — its persona and privacy are the point". The briefer
//   reads the owner's unread notifications, their queues, their DMs and their
//   plans. Letting an admin point it at an org-shared model would quietly route
//   one person's private attention state through somebody else's chosen brain,
//   and no amount of prompt care makes that acceptable.
//
//   So `model` below declares an EMPTY chain rather than a fallback: there is
//   no correct second choice. Production always supplies the owner's assistant
//   as `RunContext.model`, and the fitness suite pins its candidate the same
//   way. This is deliberately stricter than the Inbox trio, which declares
//   `chain: ['utility', 'first-routable']` so it can still be enumerated on an
//   install with no assistant configured — those harnesses have a sensible
//   degraded answer and this one does not. A briefing written by a model the
//   owner never chose is not a degraded briefing; it is a privacy defect.
//
// EPHEMERAL BY DESIGN, and preserved. The briefing lives in one `briefings` row
// that is replaced on regeneration, and the chat-back writes NOTHING — no
// conversation row, no messages, nothing to distill later. That is why only the
// briefing declares `redact` and the chat does not: redaction is about the
// SAVED copy, and the chat has none.
import { belowAnswerFloor, defineHarness, type Message } from '../define'

/** The console views a briefing can be written for. Lives here rather than in
 *  `server/briefing.ts` because `render` below is what actually varies by
 *  scope; `briefing.ts` re-exports it, so every existing caller is unaffected. */
export type BriefingScope = 'inbox' | 'boards' | 'comms' | 'plans' | 'research'

/** Each console view gets its OWN prompt: what the view is, which verbs matter
 *  there, and what an empty state means — not one template with a swapped noun.
 *  Preserved verbatim from `generateBriefing`. */
const SCOPE_PROMPT: Record<BriefingScope, { intro: string; guidance: string; empty: string }> = {
  inbox: {
    intro:
      'You are briefing your owner as they open their INBOX — the cross-workspace view of everything unreviewed: notifications, work queues, approvals, unread rooms.',
    guidance: 'Lead with whatever is most time-sensitive regardless of area. Recommend the single next action when one is obvious ("approve", "triage", "read").',
    empty: 'Nothing is waiting on them anywhere. One short line saying they are all clear.',
  },
  boards: {
    intro: 'You are briefing your owner on the BOARDS view — ticket work waiting on a human: triage (assign or park), review (sign off agent work), and unblocking.',
    guidance: 'Think like a delivery lead: sizes of each queue, what is aging, what one triage or review pass would clear. Suggest where to start.',
    empty: 'No tickets are waiting on them — queues are clear. One short line.',
  },
  comms: {
    intro: 'You are briefing your owner on their COMMS — unread channels, relays, DMs, and mentions.',
    guidance: 'Who is waiting on a reply matters most: name people and rooms, distinguish a direct question from ambient chatter. Suggest which thread to open first.',
    empty: 'No unread conversations, nobody waiting on a reply. One short line.',
  },
  plans: {
    intro: 'You are briefing your owner on their PLANS — living planning docs, some being actively worked by agents right now, some newly shared with them.',
    guidance: 'Call out plans in motion RIGHT NOW first, then fresh shares they have not looked at. Frame as "what moved since you last looked".',
    empty: 'No plan activity since they last looked. One short line.',
  },
  research: {
    intro: 'You are briefing your owner on RESEARCH — runs in flight and finished reports they have not read yet.',
    guidance: 'Ready-and-unread reports first (that is the payoff), then what is still running. Mention what each ready report answers, not just its title.',
    empty: 'No research running and nothing unread. One short line.',
  },
}

// ── 1. The briefing ──────────────────────────────────────────────────────────

export interface BriefingInput {
  scope: BriefingScope
  /** The compact human-readable attention lines, already assembled and bounded
   *  by `attentionState`. The harness never touches the database. */
  lines: string[]
  empty: boolean
}

/** The shape rules, verbatim. Everything a small model routinely drops is in
 *  here — the bullet cap, the one-line-each cap, the ordering, the grouping. */
const RULES =
  'Ground every line in the data below ONLY. Rules: at most 5 bullets, one short line each, most urgent first, lead word bolded. No preamble, no sign-off, no tools, no invented items. Group similar items ("3 research briefs ready") instead of listing each.'

/** The widened addition, and note what it is NOT: it does not raise the bullet
 *  cap, relax the grounding rule, or hand the model anything it could not
 *  already see. It asks for the SYNTHESIS a briefing is really for — what these
 *  items add up to and what to do first — which is exactly the thing a 7B model
 *  answers by restating the input lines back in a different order.
 *
 *  It is gated rather than default because synthesis is where a weak model
 *  invents: asked what two items have in common, it finds something. The
 *  "where they are not, do not connect them" clause is the instruction that has
 *  to hold, and holding it is what `instruction-following` measures. */
const SYNTHESIS =
  'Open with ONE short lead line saying what this adds up to and what to do first, then the bullets. Where two items are the same problem, say so; where they are not, do not connect them.'

const briefingPrompt = (input: BriefingInput, widened: boolean): string => {
  const spec = SCOPE_PROMPT[input.scope]
  const head = `[Automated ${input.scope} briefing — no human sent this.]\n${spec.intro}\n`
  if (input.empty) return `${head}${spec.empty} No tools, no preamble.`
  return `${head}${spec.guidance}\n${RULES}${widened ? ` ${SYNTHESIS}` : ''}\n\n${input.lines.map((l) => `- ${l}`).join('\n')}`
}

/** A realistic attention set: one blocked ticket, one mention, one unread room.
 *  The lines are the exact shapes `attentionState` builds, so a fixture that
 *  passes here is a fixture that passed on production input. */
const FIXTURE_LINES = [
  'ticket blocked: "Ledger migration" on Platform',
  'notification (mention): Priya asked about the rollback window',
  '3 unread in #platform',
]

/** One briefed item: a list marker, or the bolded lead word the prompt asks
 *  for. Both spellings count because the panel renders markdown either way and
 *  the instruction being checked is "at most 5, one short line each" rather
 *  than a preference for hyphens.
 *
 *  A widened briefing's opening lead line is plain prose with no marker and no
 *  bold lead, so it deliberately does NOT count — which is what keeps the cap
 *  the same assertion under both surfaces. */
const ITEM_LINE = /^\s*(?:[-*+]\s+|\d+[.)]\s+|\*\*)/
const itemLines = (value: string): string[] => value.split('\n').filter((l) => ITEM_LINE.test(l))

/** A link or a UUID in a briefing is INVENTED by construction: nothing in
 *  `attentionState`'s lines carries either. This is `ungrounded_ref`'s question
 *  asked deterministically, on a transport that cannot answer it — the persona
 *  stream gives tool names and no results, so the real rule is skipped here
 *  (see the guard block below), and the eval is where the fixture gets to
 *  check it anyway. */
const INVENTED_REF = /https?:\/\/|\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i

export const briefingHarness = defineHarness<BriefingInput, string>({
  id: 'briefer:brief',
  label: 'Briefer',
  job: 'Writes the short read on what needs the owner’s attention in one console view.',
  // No JSON here — the output is the markdown the panel renders. What the job
  // leans on is holding six formatting and grounding constraints in one turn,
  // which is what `instruction-following` measures and what a 7B model drops
  // first (it lists ten items instead of five, and restates rather than groups).
  requires: ['instruction-following'],
  floor: {
    // Nothing refuses. A looser briefing is still a briefing, and the panel it
    // feeds is a convenience over consoles that already show every item
    // themselves — the owner loses a paragraph, not their inbox. Empty refusal
    // list because `runHarness` reads it only when `refuseBelow` is true.
    capabilities: [],
    refuseBelow: false,
    note: 'A smaller model writes a longer, flatter briefing; the consoles underneath still list every item themselves.',
  },
  // See the header: the empty chain is the point. The owner's own assistant
  // arrives as `RunContext.model` and there is no acceptable second choice.
  model: { chain: [] },
  render: (input, ctx): Message[] => [{ role: 'user', content: briefingPrompt(input, ctx.widened) }],
  output: { kind: 'text', clean: (raw) => raw.trim() || null },
  // The caller turns this into a throw so its `.catch` clears the `generating`
  // flag — see `generateBriefing`. It stays 'null' here rather than 'throw'
  // because `runHarness` RETURNS rather than throws when no model resolves at
  // all, so 'throw' would cover one of the two failure modes and silently skip
  // the other. The previous summary is left in place either way, which is the
  // behavior that already existed: a failed regeneration shows stale, never blank.
  onFailure: 'null',
  widen: {
    // The same capability the job already leans on, because the widened prompt
    // is not a different ask — it is the SAME ask with one more constraint on
    // it, and the constraint ("where they are not, do not connect them") is the
    // one that keeps synthesis from becoming invention. A model that has proved
    // it honors an explicit negative instruction is exactly the model that can
    // be trusted to leave two unrelated items unconnected.
    requires: ['instruction-following'],
    note: 'Models proven to honor an explicit "do not connect unrelated items" instruction also open the briefing with what it all adds up to and what to do first.',
  },
  guard: {
    // NOT `zero_tool_claim`, and the reason is the same one the Concluder and
    // the Distiller give: a briefing REPORTS what happened in the owner's
    // workspace, so it is full of sentences like "Priya sent 3 messages" and
    // "2 tickets moved to review" — which is `CLAIM_VERB_ART` firing on the
    // harness doing its job exactly right. Findings that fire on correct output
    // are not a safety net, they are noise in `guard_findings.model`, which is
    // the per-model confabulation rate the fitness page reads next to benched
    // scores.
    //
    // NOT `fabricated_outage` either: the attention lines literally include
    // 'plan FAILED — its last turn errored' and 'research FAILED — needs a
    // re-run'. Reporting a real failure is the job. (It would be skipped on the
    // persona transport regardless, since a stream carries no error detail —
    // declaring it would be theatre.)
    rules: ['secret_leak', 'pii_leak'],
    // The briefing is PERSISTED to `briefings.summary` and re-rendered on every
    // poll until the attention fingerprint changes. Its source material is
    // notification titles, ticket titles and channel names — text people paste
    // credentials into — so an echoed key would outlive the incident that
    // caused it. This is the redaction case guardrails.ts's header describes.
    redact: true,
  },
  evals: [
    {
      // The formatting contract, checked under BOTH surfaces: the widened
      // prompt adds a lead LINE, not a sixth bullet, so the cap is the same
      // assertion either way and this fixture never has to know which it got.
      name: 'keeps to the five-bullet shape it was asked for',
      input: { scope: 'inbox', lines: FIXTURE_LINES, empty: false },
      check: (value) => {
        if (!value.trim()) return 'the briefing was empty'
        const items = itemLines(value)
        if (items.length === 0) return 'the briefing came back as prose with no bulleted items'
        if (items.length > 5) return `wrote ${items.length} bullets, over the 5 the prompt allows`
        const long = items.find((b) => b.length > 200)
        return long ? `a bullet ran to ${long.length} chars, well past "one short line each"` : null
      },
    },
    {
      name: 'grounds the briefing in the items it was given and invents no references',
      input: { scope: 'inbox', lines: FIXTURE_LINES, empty: false },
      check: (value) => {
        // The blocked ticket is the only genuinely urgent item in the fixture,
        // and every scope prompt says "most urgent first". A briefing that
        // never names it has not read its input.
        if (!value.toLowerCase().includes('ledger')) return 'never mentioned "Ledger migration", the one blocked ticket it was given'
        const ref = INVENTED_REF.exec(value)
        return ref ? `cited "${ref[0]}" — nothing in the attention lines carries a link or an id, so it was invented` : null
      },
    },
    {
      // The empty state is the sharpest small-model tell in this harness: asked
      // for "one short line saying they are all clear", a weak model writes a
      // reassuring paragraph with a bulleted list of the things that are NOT
      // waiting. That is the whole panel filled with nothing.
      name: 'answers the all-clear state with one short line',
      input: { scope: 'inbox', lines: [], empty: true },
      check: (value) => {
        if (!value.trim()) return 'the all-clear briefing was empty'
        // A FLOOR AS WELL AS A CEILING. Everything else here is an upper bound,
        // so a two-word non-answer scored a pass on the fixture whose whole
        // subject is what a model says when there is nothing to say. `all
        // clear` / `nothing` / `caught up` is the range of honest answers, and
        // the alternatives are broad enough not to grade wording.
        const thin = belowAnswerFloor(value, { minChars: 12, mentions: ['clear', 'nothing', 'caught up', 'no ', 'quiet', 'empty'] })
        if (thin) return thin
        if (itemLines(value).length > 0) return 'wrote a bulleted list for a state with nothing in it'
        return value.length <= 200 ? null : `wrote ${value.length} chars where one short line was asked for`
      },
    },
  ],
})

// ── 2. The chat-back ─────────────────────────────────────────────────────────

export interface BriefingChatInput {
  scope: BriefingScope
  /** The briefing currently on screen, or null before the first one lands. */
  summary: string | null
  history: Array<{ role: 'user' | 'assistant'; content: string }>
  /** What the owner just typed. */
  content: string
}

/** The last dozen turns, which is what the caller has always sent. Stated here
 *  rather than at the call site because it is a prompt-shaping decision. */
const HISTORY_TURNS = 12

export const briefingChatHarness = defineHarness<BriefingChatInput, string>({
  id: 'briefer:chat',
  label: 'Briefing chat',
  job: 'Answers the owner’s follow-up questions about the briefing on screen, saving nothing.',
  requires: [],
  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Runs on anything the owner’s assistant runs on; a plainer answer from a small model is still an answer.',
  },
  // Same fixed model as the briefing above, for the same reason — this thread
  // is the owner asking about their own unread work.
  model: { chain: [] },
  render: (input): Message[] => [
    {
      role: 'user',
      content:
        `[Ephemeral briefing chat — this thread is NOT saved. Keep replies short and direct; use tools only if the owner's question truly needs them.]\n` +
        `Your latest briefing to your owner:\n${input.summary ?? '(none yet)'}`,
    },
    { role: 'assistant', content: 'Got it.' },
    ...input.history.slice(-HISTORY_TURNS),
    { role: 'user', content: input.content },
  ],
  output: { kind: 'text', clean: (raw) => raw.trim() || null },
  // Nothing is persisted and the owner already has the streamed answer on
  // screen, so there is nothing for a failure policy to salvage. The value
  // exists for the guard pass and the run row.
  onFailure: 'null',
  guard: {
    // `zero_tool_claim` DOES run here, unlike the briefing above, and the
    // difference is real rather than fussy: this is a chat reply, the prompt
    // explicitly permits tools ("use tools only if the owner's question truly
    // needs them"), and the identical question asked in ordinary chat is
    // already checked by `guardChatReply` with this rule on. Leaving it off
    // would make the briefing panel the one place in the product where "I've
    // archived those for you" goes unnoticed. The persona stream reports tool
    // NAMES, which is precisely what this rule needs and all it needs.
    rules: ['zero_tool_claim', 'secret_leak', 'pii_leak'],
    // NO `redact`, and this is a decision rather than an omission. The exchange
    // is deliberately ephemeral — no conversation row, no messages, nothing to
    // distill later — so there is no saved copy to clean, and the owner has
    // already watched the original stream token by token. Redacting a value
    // nobody reads would buy nothing and would misreport the run as repaired.
    // What the guard is for HERE is the finding: `guard_findings.model` is the
    // live confabulation rate the fitness page reads.
  },
  // The prompt says "use tools only if the owner's question truly needs them",
  // and it means it: "what's blocking t-41?" is answered from live data or not
  // at all. Suppressing them would answer it from memory, which on this panel
  // reads as a confident wrong answer rather than as a missing feature. This
  // declaration is half of what deleted the tee transport in server/briefing.ts;
  // the other half is `runHarnessStreamed`, because STREAMING IS ALSO THE
  // FEATURE here — the owner watches the reply arrive — and streaming is a
  // property of the transport, never of this contract.
  tools: 'own',
  // Thirty seconds, not `proxyChat`'s two minutes: a person is watching a
  // spinner. A work session can afford to hold ten minutes for a restarting
  // agent (see harness/defs/work-session.ts); this panel cannot.
  holdMs: 30_000,
  evals: [
    {
      name: 'answers from the briefing it was given',
      input: {
        scope: 'inbox',
        summary: '**Blocked** — "Ledger migration" on Platform is blocked.\n**Unread** — 3 messages in #platform.',
        history: [],
        content: 'which one is blocked?',
      },
      check: (value) => {
        if (!value.trim()) return 'the assistant returned nothing'
        return value.toLowerCase().includes('ledger') ? null : 'did not name "Ledger migration", the only blocked item in the briefing it was shown'
      },
    },
    {
      name: 'keeps the reply short and direct',
      input: {
        scope: 'boards',
        summary: '**Review** — 2 tickets are waiting on your sign-off on Platform.',
        history: [],
        content: 'what should I do first?',
      },
      check: (value) => {
        const text = value.trim()
        // The 10-character floor was too low to reject a non-answer: the
        // question is "what should I do first?" over a briefing about tickets
        // waiting on a sign-off, and an answer that engages with none of that
        // is not short and direct, it is absent.
        const thin = belowAnswerFloor(text, { minChars: 20, mentions: ['review', 'sign', 'ticket', 'platform', 'approve', 'waiting'] })
        if (thin) return thin
        // "Short and direct" is the instruction; an essay is the small-model
        // failure. Generous enough that a thorough answer passes and an
        // unbounded one does not.
        return text.length <= 1200 ? null : `wrote ${text.length} chars where the prompt asks for a short, direct reply`
      },
    },
  ],
})
