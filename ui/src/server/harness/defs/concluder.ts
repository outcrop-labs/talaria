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
const NARROW =
  'Write the closing summary for a work discussion: what was decided, what was produced, and any follow-ups — crisp markdown, a few bullets per section, no preamble.'

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
  evals: [
    {
      name: 'comes back as sections rather than a paragraph',
      input: { channelName: 'pilot-export', transcript: FIXTURE },
      check: (value) => {
        if (!value.trim()) return 'the summary was empty'
        const lines = structuredLines(value)
        return lines >= 3 ? null : `expected at least 3 bullet or heading lines, got ${lines} - the summary came back as prose`
      },
    },
    {
      name: 'carries the decision, the deliverable and the follow-up',
      input: { channelName: 'pilot-export', transcript: FIXTURE },
      check: (value) => {
        const v = value.toLowerCase()
        const missing = (
          [
            ['csv', 'the decision to ship CSV only'],
            ['endpoint', 'the endpoint that was produced'],
            ['thursday', 'the follow-up and its deadline'],
          ] as const
        )
          .filter(([token]) => !v.includes(token))
          .map(([, label]) => label)
        return missing.length ? `left out ${missing.join(', ')}` : null
      },
    },
  ],
})
