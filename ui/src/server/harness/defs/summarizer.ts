// The Summarizer harness: one plain line per agent skill, saying what it
// teaches. Shown under the skill's title everywhere skills are listed, and
// persisted keyed to a hash of the SKILL.md, so this runs once per skill
// version and never on a read path.
//
// WHAT MOVED HERE FROM skill-summaries.ts
//   - the model chain (pin -> utility -> env -> 'pl-main' -> first routable),
//     which was a VERBATIM copy of the same eight lines in titler.ts,
//     model-info.ts and kb-okf.ts (audit 1.10). It is now a ModelSpec.
//   - the first-non-empty-line extraction, which is the real output contract of
//     this harness and had been living as a chain of optional calls inside a
//     fire-and-forget IIFE where nothing could test it. It is `clean` below.
//   - the catch-and-return-null, which is `onFailure: 'null'`.
//
// FIRE-AND-FORGET IS THE POINT, and it survives the port unchanged: when the
// model gives us nothing usable, the caller writes nothing and the stored
// summary from the previous version stays on screen. A stale line beats a
// garbage line, and a skill whose summary failed is re-queued the next time
// anything lists it.
import { belowAnswerFloor, defineHarness } from '../define'
import { firstMeaningfulLine } from '../text'

/** What the model is asked for. Unchanged from the pre-harness prompt, down to
 *  the wording: this is the one thing in the port that a model can notice, and
 *  changing the prompt and the plumbing in the same commit would make any
 *  quality change impossible to attribute. */
const PROMPT =
  'Summarize this agent skill in ONE sentence (max 140 chars): what kind of work it covers and the gist of how. ' +
  'Plain words, no markdown, no "This skill…" lead-in — start with the substance. Reply with ONLY the sentence.'

/** How much of a SKILL.md the model sees. A skill document can be tens of
 *  thousands of characters and the gist is always in its opening; sending the
 *  whole thing to a 14B model with an 8k window costs the instruction, which is
 *  the one part of the prompt that has to survive. */
const MAX_INPUT = 6000

/** Hard clamp on the stored line. The prompt asks for 140; this is the width
 *  the Studio's one-line slot can render without wrapping, and it is deliberately
 *  LOOSER than the prompt so that a model which overshoots by a few words still
 *  produces a usable summary instead of nothing. */
const MAX_SUMMARY = 180

export interface SummarizerInput {
  /** The skill's SKILL.md, as written. Truncation is this harness's business. */
  md: string
}

/** The text contract. The extraction itself is `firstMeaningfulLine`
 *  (harness/text.ts) — this file used to carry its own copy of it, one
 *  character different from the titler's, and that copy stored the literal
 *  "```" for a fenced reply and kept a trailing "**" on a bolded one. Both are
 *  fixed in the shared helper; all that is left here is the width clamp.
 *
 *  Returning null when nothing survives is what keeps the previous summary on
 *  screen instead of overwriting it with an empty string. */
function firstLine(raw: string): string | null {
  const line = firstMeaningfulLine(raw)
  return line === null ? null : line.slice(0, MAX_SUMMARY) || null
}

const noMarkdown = (s: string): boolean => !/[`*]|\[[^\]]*\]\(/.test(s)

export const summarizerHarness = defineHarness<SummarizerInput, string>({
  id: 'summarizer',
  label: 'Summarizer',
  job: 'Keeps the Studio readable: one plain line per skill saying what it teaches, regenerated only when the skill changes.',

  // Nothing. This is one sentence of prose out of one document of prose — no
  // JSON, no tools, no search, no long context (the input is clamped above).
  requires: [],

  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Runs on any model the gateway serves; a weak one writes a duller line, and a duller line is still better than a skill with no summary at all.',
  },

  // pin: the admin's Models -> Platform assignment for this agent. Everything
  // after it — the Utility role, the env default, the first routable bare model
  // — is the DEFAULT chain's business, not this file's. No `role: 'utility'`:
  // the default chain already has a 'utility' step, and declaring the role too
  // would resolve the same model one step earlier under a different label
  // (`harness_runs.chain_step`), which is the one thing the step is recorded
  // for. See the same note in titler.ts.
  model: { pin: 'summarizer' },

  render: (input) => [
    { role: 'system', content: PROMPT },
    { role: 'user', content: input.md.slice(0, MAX_INPUT) },
  ],

  output: { kind: 'text', clean: firstLine },

  // The caller keeps the summary it had. See the note at the top of this file:
  // this is the property that makes a bad model a duller Studio rather than a
  // broken one.
  onFailure: 'null',

  // A skill summary is PERSISTED and shown to everyone who can see the skill,
  // so a credential that a SKILL.md carried in an example block and the model
  // helpfully echoed must not be written to the row — hence `redact`.
  //
  // The rule list is narrowed on purpose. `zero_tool_claim` and
  // `fabricated_outage` read a DESCRIPTION of work as a CLAIM of work: a
  // faithful summary of a ticket-filing skill says "tickets are created", which
  // is that rule's exact pattern, with no tool record to ground it because a
  // summarizer turn calls no tools. Before this port those false positives went
  // into `guard_findings` under this model's name and inflated the very
  // confabulation rate the fitness page reads next to its benchmark scores.
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },

  temperature: 0.3,

  // Widening: none, deliberately. A frontier model writes a better sentence than
  // a 14B one and it does so from this same prompt — there is no extra thing to
  // let it DO here, and inviting it to say more would only make the line wrap.

  evals: [
    {
      name: 'ordinary skill document',
      input: {
        md: [
          '# Release notes',
          '',
          'Use this skill when a milestone closes and the changelog needs writing.',
          '',
          '## Steps',
          '1. Collect the merged PR titles since the last tag.',
          '2. Group them into Added / Fixed / Changed.',
          '3. Post the result to the release channel.',
        ].join('\n'),
      },
      check: (value) => {
        // THE FLOOR FIRST. Every assertion under it is a NOT — not long, not
        // markdown, not the forbidden lead-in — and a 14-character non-answer
        // satisfies all of them. See `belowAnswerFloor`.
        const thin = belowAnswerFloor(value, { minChars: 25, mentions: ['release', 'changelog', 'pr', 'milestone', 'note'] })
        if (thin) return thin
        if (value.includes('\n')) return 'the summary is more than one line'
        if (value.length > 140) return `the summary is ${value.length} characters; the prompt asks for 140 or fewer`
        if (!noMarkdown(value)) return 'the summary contains markdown'
        if (/^this skill\b/i.test(value)) return 'the summary opens with the "This skill…" lead-in the prompt forbids'
        return null
      },
    },
    {
      name: 'skill whose document opens with a fenced code block',
      // The shape that used to defeat this harness: the model mirrors the input
      // and answers with a fence, or with a heading, and a summary that keeps
      // the decoration renders as literal asterisks in the Studio.
      input: {
        md: [
          '```bash',
          'talaria deploy --env staging',
          '```',
          '',
          '# Staging deploys',
          '',
          'Run a staging deploy, watch the health checks, and roll back automatically if any check fails within ten minutes.',
        ].join('\n'),
      },
      check: (value) => {
        if (!noMarkdown(value)) return 'the summary carried markdown out of the document'
        if (value.length > 140) return `the summary is ${value.length} characters; the prompt asks for 140 or fewer`
        if (value.trim().length < 15) return 'the summary is too short to say what the skill covers'
        return null
      },
    },
    {
      name: 'terse skill document',
      // Almost nothing to work with. The failure to catch is the model padding
      // its way to a paragraph, or answering with a question, rather than
      // summarizing what little is there.
      input: { md: '# Tag bug reports\n\nLabel incoming bug reports by the component they mention.' },
      check: (value) => {
        // The document is two lines long, so the floor is the whole assertion
        // worth making: an answer that engages with neither bugs nor labels is
        // not a summary of it, however well-formed it looks.
        const thin = belowAnswerFloor(value, { minChars: 20, mentions: ['bug', 'label', 'tag', 'component', 'report'] })
        if (thin) return thin
        if (value.includes('\n')) return 'the summary is more than one line'
        if (value.length > 140) return `the summary is ${value.length} characters; the prompt asks for 140 or fewer`
        if (value.trim().endsWith('?')) return 'the summary is a question rather than a summary'
        if (/^this skill\b/i.test(value)) return 'the summary opens with the "This skill…" lead-in the prompt forbids'
        return null
      },
    },
  ],
})
