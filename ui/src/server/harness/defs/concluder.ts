// The Concluder: a relay's last word.
//
// Unlike the Distiller, this one is USER-INITIATED — somebody clicked "conclude"
// and is watching a spinner — so its failures are visible and its caller
// (`concludeRelay`) throws user-facing copy rather than swallowing a null. That
// difference is the whole reason the two harnesses in this pair declare
// different failure handling despite doing nearly the same job.
//
// PORTED FROM comms-decay.ts (audit 1.10). Prompt, temperature and user turn are
// the originals. The hand-copied model chain and the `if (!text.trim())` check
// are gone; the runner owns both.
import { defineHarness } from '../define'

export interface ConcludeInput {
  /** The relay's name, as the participants know it. */
  channelName: string
  /** The rendered transcript, already filtered and clipped by the caller. */
  transcript: string
}

/** The original prompt, preserved verbatim. */
/** THE NARROW PROMPT, AND THE FLOOR IT WAS MISSING.
 *
 *  It used to be the first sentence alone. Two fixtures graded rules that only
 *  the WIDENED prompt stated, and the sweep runs NARROW on every model that is
 *  not widened — so most candidates were being graded on instructions they were
 *  never given:
 *
 *    `leaves an unowned follow-up unowned instead of guessing at a name` — the
 *    "leave it unattributed rather than guessing" clause lives in WIDE.
 *    `does not turn a config change into an invented deliverable` — no prompt
 *    said not to invent one. The distiller has "Never invent anything"; this
 *    harness, which summarises a transcript the same way, had nothing.
 *
 *  WHAT WIDENING STILL BUYS is unchanged, and that matters: the floor below is
 *  NEGATIVE (do not invent, do not guess an owner) while WIDE's ask is POSITIVE
 *  (attribute every decision and every follow-up by name). A model that only has
 *  the floor writes a correct unattributed summary; a widened one writes an
 *  attributed one. Moving the negative half down does not make the positive half
 *  redundant. */
const NARROW = [
  'Write the closing summary for a work discussion: what was decided, what was produced, and any follow-ups — crisp markdown, a few bullets per section, no preamble.',
  'Only what the transcript actually says: never invent a deliverable, an outcome or a date, and where it does not name who owns a follow-up, leave it unowned rather than guessing.',
].join('\n')

/** The widened prompt. The narrow one already asks for sections, so widening
 *  does not buy structure here — it buys ATTRIBUTION, which is the thing a
 *  multi-party transcript has and a single-thread chat does not. Who decided
 *  it, and who owns the follow-up, is what makes a concluded relay actionable
 *  a week later.
 *
 *  It is gated rather than default because attribution is where a weak model
 *  invents: asked for an owner on every follow-up, it assigns one, and a
 *  summary that names the wrong person as accountable is worse than a summary
 *  that names nobody. The "leave it unattributed" clause is the instruction
 *  that has to hold, and that is what the capability gate is checking for. */
const WIDE = [
  'Write the closing summary for a work discussion, in three sections: what was decided, what was produced,',
  'and any follow-ups. Crisp markdown, a few bullets per section, no preamble.',
  '',
  'Attribute each decision to whoever made it and each follow-up to whoever took it on, by the name they',
  'are given in the transcript. Where the transcript does not say, leave it unattributed rather than',
  'guessing - an unowned follow-up is useful and a misattributed one is worse than none.',
].join('\n')

/** A short relay with one decision, one deliverable and one follow-up — the
 *  three sections the prompt asks for, so a summary that omits a section has
 *  demonstrably omitted content rather than merely had nothing to say. */
const FIXTURE = [
  'Priya: We need the export endpoint before the pilot.',
  'Nomad: I have the CSV writer working. Streaming, so it holds on the big accounts.',
  'Priya: Good. Decision: we ship CSV only for the pilot, no XLSX.',
  'Nomad: Understood. I pushed the endpoint and the fixture set this morning.',
  'Priya: Follow-up - somebody has to write the customer-facing note before Thursday.',
  'Nomad: I can draft it.',
].join('\n\n')

/** Bullet or heading lines. A "few bullets per section" summary that comes back
 *  as one prose paragraph has ignored the only formatting instruction it was
 *  given, which is the reliable small-model tell on this harness. */
const structuredLines = (value: string): number => value.split('\n').filter((l) => /^\s*(?:[-*+]\s+|#{1,6}\s+|\d+[.)]\s+)/.test(l)).length

/** ONE DECISION, ONE DELIVERABLE, ONE FOLLOW-UP, and nothing to disentangle.
 *  The easy floor: a model that cannot section this cannot section anything. */
const SIMPLE = [
  'Ada: The staging certs expire Sunday.',
  'Nomad: Renewed them this morning and pushed the new bundle.',
  'Ada: Decision: we move to auto-renew so this stops happening.',
  'Nomad: Follow-up - I will wire the ACME client this week.',
].join('\n\n')

/** A relay where the DECISION IS A REVERSAL. The summary has to record what was
 *  landed on, not both halves of the argument. */
const REVERSED = [
  'Priya: I think we put the export behind a feature flag.',
  'Nomad: that would let us dark-launch it.',
  'Priya: on reflection no - the flag is more moving parts than the feature. Decision: ship it unflagged to the pilot accounts only.',
  'Nomad: unflagged, pilot accounts. Pushed the allowlist.',
  'Priya: Follow-up - review the allowlist with Support on Monday.',
].join('\n\n')

/** A relay with UNATTRIBUTED follow-ups: nobody says who is doing it. The
 *  widened prompt's hardest rule is "leave it unattributed rather than
 *  guessing", and this is where a weak model invents an owner. */
const UNOWNED = [
  'Marta: The importer times out on files over 200MB.',
  'Nomad: confirmed, it is the synchronous parse.',
  'Marta: Decision: we chunk the parse rather than raising the timeout.',
  'Nomad: I have the chunker prototyped.',
  'Marta: Follow-up - someone needs to benchmark it against the worst customer file before we ship.',
].join('\n\n')

/** A relay that produced NO deliverable — a discussion that decided something
 *  and built nothing. Inventing a deliverable to fill the section is the
 *  failure. */
const NO_DELIVERABLE = [
  'Ivan: Are we keeping the nightly rebuild?',
  'Nomad: it costs about forty minutes of runner time a day.',
  'Ivan: Decision: drop it, move to on-demand rebuilds from the release branch.',
  'Nomad: nothing to build for that, it is a config change on the schedule.',
  'Ivan: Follow-up - Ivan will delete the schedule after the current train ships.',
].join('\n\n')

export const concluderHarness = defineHarness<ConcludeInput, string>({
  id: 'concluder',
  label: 'Concluder',
  job: 'Writes the closing summary when a relay concludes — decisions, deliverables, follow-ups.',
  // Same reasoning as the distiller: the transcript is clipped, so the ask is
  // bounded, and what the job actually leans on is holding several formatting
  // and content constraints in one system turn.
  requires: ['instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    // Refusing would be the wrong trade even though this one is user-facing: a
    // loose summary is still a record, and the person is right there to read it
    // and re-run. Failure is already visible here — `concludeRelay` throws and
    // the UI shows the message — so there is nothing for a floor to protect.
    note: 'A smaller model writes a looser summary; if it returns nothing at all the relay stays open and you can conclude it again.',
  },
  // The chain comms-decay hand-wrote: the Concluder's assigned model, else the
  // concluding user's own muse. `userId` on the run context turns on the
  // 'preferred' step and the member model allowlist.
  model: { pin: 'concluder', chain: ['pin', 'preferred', 'utility', 'env', 'first-routable'] },
  render: (input, ctx) => [
    { role: 'system', content: ctx.widened ? WIDE : NARROW },
    { role: 'user', content: `Relay "${input.channelName}":\n\n${input.transcript}` },
  ],
  output: { kind: 'text', clean: (raw) => raw.trim() || null },
  // NOT 'throw'. `runHarness` returns rather than throws when no model resolves
  // at all, so 'throw' would cover one of this caller's two failure modes and
  // silently skip the other. `concludeRelay` distinguishes them itself — a null
  // MODEL means "nothing is configured to summarize with", a null VALUE means
  // "it was asked and answered with nothing" — and those are different
  // sentences for the person watching the spinner.
  onFailure: 'null',
  widen: {
    requires: ['instruction-following'],
    note: 'Models proven to follow a "leave it unattributed rather than guess" instruction also name who decided what and who owns each follow-up.',
  },
  // Narrowed for the same reason as the distiller: a closing summary REPORTS
  // what agents did and what broke, so `zero_tool_claim` and
  // `fabricated_outage` fire on it doing its job. `redact` is on because this
  // text is posted back into the channel and indexed for retrieval, so a
  // credential quoted out of the transcript would outlive the relay in two
  // places at once.
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  temperature: 0.2,
  // NINE FIXTURES, THREE BANDS. Both originals ran on the same transcript, so a
  // model that happened to section THAT relay scored a perfect concluder. The
  // shapes below are the ones that break the job differently: a reversal, a
  // relay with nobody named, a relay that produced nothing, and one long enough
  // that keeping every section takes actually reading it.
  evals: [
    {
      name: 'sections a short, unambiguous relay',
      band: 'easy',
      input: { channelName: 'certs', transcript: SIMPLE },
      check: (value) => sectionProblem(value) ?? carries(value, [['cert', 'the expiring certs'], ['renew', 'the renewal decision']]),
    },
    {
      name: 'comes back as sections rather than a paragraph',
      band: 'easy',
      input: { channelName: 'pilot-export', transcript: FIXTURE },
      check: (value) => sectionProblem(value),
    },
    {
      name: 'carries the decision, the deliverable and the follow-up',
      band: 'standard',
      input: { channelName: 'pilot-export', transcript: FIXTURE },
      check: (value) =>
        carries(value, [
          ['csv', 'the decision to ship CSV only'],
          ['endpoint', 'the endpoint that was produced'],
          ['thursday', 'the follow-up and its deadline'],
        ]),
    },
    {
      name: 'keeps all three sections on a relay with plenty to lose',
      band: 'standard',
      input: { channelName: 'importer', transcript: UNOWNED },
      check: (value) =>
        sectionProblem(value) ??
        carries(value, [
          ['chunk', 'the decision to chunk the parse'],
          ['benchmark', 'the follow-up'],
        ]),
    },
    {
      name: 'does not turn a config change into an invented deliverable',
      band: 'standard',
      input: { channelName: 'ci', transcript: NO_DELIVERABLE },
      check: (value) => {
        const problem = sectionProblem(value) ?? carries(value, [['on-demand', 'the decision'], ['schedule', 'the follow-up']])
        if (problem) return problem
        // The relay explicitly says there is nothing to build. A summary that
        // reports shipped code has invented the one thing nobody did.
        return /\b(?:shipped|merged|deployed|released|built the)\b/i.test(value)
          ? 'reported a deliverable on a relay whose participants said there was nothing to build'
          : null
      },
    },
    {
      name: 'names the channel’s subject rather than restating the transcript',
      band: 'standard',
      input: { channelName: 'pilot-export', transcript: FIXTURE },
      // THE FLOOR FIRST. On its own this is a pure upper bound, and a
      // fourteen-character non-answer is shorter than any transcript — the
      // one-sided assertion the sweep's own garbage census exists to catch, and
      // it caught this one in draft.
      check: (value) =>
        sectionProblem(value) ??
        carries(value, [['csv', 'the decision']]) ??
        (value.length < FIXTURE.length ? null : 'the summary is no shorter than the relay it closes — the model restated it rather than concluding it'),
    },
    // ── hard ────────────────────────────────────────────────────────────────
    {
      name: 'records the position the relay landed on, not the one it abandoned',
      band: 'hard',
      input: { channelName: 'pilot-export', transcript: REVERSED },
      check: (value) => {
        const problem = sectionProblem(value) ?? carries(value, [['unflagged', 'the decision that was actually taken'], ['allowlist', 'the deliverable']])
        if (problem) return problem
        // Naming the flag as the rejected option is fine; presenting it as the
        // decision is the failure.
        return /decision[^\n]*feature flag|ship[^\n]*behind (?:a )?(?:feature )?flag/i.test(value)
          ? 'recorded the reversed decision (the feature flag) as if it still stood'
          : null
      },
    },
    {
      name: 'leaves an unowned follow-up unowned instead of guessing at a name',
      band: 'hard',
      // The widened prompt's hardest clause. Nobody in this relay takes the
      // benchmark, and a model asked for an owner on every follow-up will
      // assign one — which is worse than naming nobody.
      input: { channelName: 'importer', transcript: UNOWNED },
      check: (value) => {
        const followUp = value
          .split('\n')
          .filter((l) => /benchmark/i.test(l))
          .join(' ')
        if (!followUp) return 'left out the follow-up entirely'
        // Marta and Nomad are the only names in the transcript, and neither
        // volunteered for it.
        const invented = /\b(?:Marta|Nomad)\b/.exec(followUp)
        return invented ? `attributed the unowned follow-up to ${invented[0]}, who never took it on` : null
      },
    },
    {
      name: 'sections a relay whose decision is buried mid-thread',
      band: 'hard',
      input: {
        channelName: 'billing',
        transcript: [
          'Sam: the invoice PDF renders wrong on Windows.',
          'Nomad: it is the font fallback — the embedded subset is missing on their reader.',
          'Sam: how bad?',
          'Nomad: cosmetic, but it looks unprofessional on the total line.',
          'Sam: fine. Decision: we embed the full font rather than the subset, and eat the extra 400KB per invoice.',
          'Nomad: done, pushed the change and regenerated last month’s invoices.',
          'Sam: anything else outstanding?',
          'Nomad: no.',
          'Sam: Follow-up - tell Support the old PDFs are being regenerated so they stop getting tickets about it.',
        ].join('\n\n'),
      },
      check: (value) =>
        sectionProblem(value) ??
        carries(value, [
          ['font', 'the decision about the embedded font'],
          ['support', 'the follow-up to Support'],
        ]),
    },
  ],
})

// ── Eval assertions ──────────────────────────────────────────────────────────

/** THE FORMATTING HALF, with a floor under it. `structuredLines` alone is an
 *  upper-bound-free NOT: three bullets saying nothing satisfies it. The length
 *  floor is what makes it an assertion about a summary rather than about
 *  punctuation. */
function sectionProblem(value: string): string | null {
  if (!value.trim()) return 'the summary was empty'
  if (value.trim().length < 60) return `the summary is ${value.trim().length} characters — too short to carry three sections`
  const lines = structuredLines(value)
  return lines >= 3 ? null : `expected at least 3 bullet or heading lines, got ${lines} - the summary came back as prose`
}

/** THE CONTENT HALF: the load-bearing tokens of this relay, each with the
 *  human sentence naming what its absence means. */
function carries(value: string, wanted: ReadonlyArray<readonly [string, string]>): string | null {
  const v = value.toLowerCase()
  const missing = wanted.filter(([token]) => !v.includes(token)).map(([, label]) => label)
  return missing.length ? `left out ${missing.join(', ')}` : null
}
