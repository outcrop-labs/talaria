// The LIBRARIAN: the agent-facing OKF summary for one promoted knowledge doc.
//
// THE OUTPUT SHAPE, AND WHY IT IS NOT JSON
//   This harness returns a hybrid — a markdown body plus a trailing
//   `TAGS: a, b, c` line — and the audit counted that trailing line as a sixth
//   structured-output extractor (1.1). It was right to. The fix is not
//   automatically "make it JSON", though, and this is the one harness in the
//   port where the small-model tradeoff actually cuts the other way:
//
//     - The product here IS the prose. A judge's verdict is an enum that moves a
//       ticket; the librarian's value is a multi-paragraph markdown body with a
//       heading and a bullet list in it. Putting that inside a JSON string means
//       a 7-14B model has to escape newlines and quotes correctly for hundreds
//       of tokens with no delimiter to recover from — the single most reliable
//       way to make a small model fail a contract it could otherwise satisfy.
//     - The failure is also the most expensive one to repair. `runHarness`'s
//       repair turn costs a full regeneration of that same long body, on a model
//       already chosen for being cheap, for a subsystem that runs on every save.
//     - The structured part is tiny and non-fatal: up to five topic tags. A
//       model that misses the TAGS line loses the tags and keeps the summary,
//       which is a graceful degradation. A model that mis-escapes a JSON string
//       loses everything.
//
//   So: `output: { kind: 'text', clean }`, with the tag parse inside `clean`
//   returning a real typed value. That is not a sixth extractor coming back —
//   the difference between this and the line in kb-okf.ts it replaces is that
//   this one is the ONLY copy, the runner owns when it is called and what a null
//   from it means, and it is scored by the eval fixtures at the bottom of this
//   file. If the librarian ever needs nested output (per-fact provenance, say),
//   that is the moment it moves to a schema, and the cost of the move is this
//   function.
import { defineHarness } from '../define'

/** The parsed librarian reply. */
export interface LibrarianOkf {
  /** The OKF concept body: prose summary then a "## Key facts" list. The TAGS
   *  contract line has been consumed and is never part of this. */
  body: string
  /** Up to five lowercase-kebab topic tags. EMPTY IS VALID — a model that
   *  omitted the line still wrote a usable summary, and dropping the summary
   *  over a missing garnish is the wrong trade (see the header). */
  tags: string[]
}

export interface LibrarianInput {
  title: string
  body: string
}

/** How much of the document the model is shown. The narrow number is what
 *  shipped; the wide one is the widening (see `widen` below) and is the whole
 *  substance of it — a model that can hold the document extracts facts from the
 *  document, not from its first few pages. */
const NARROW_CLIP = 12_000
const WIDE_CLIP = 48_000

const clip = (s: string, max: number): string => (s.length > max ? `${s.slice(0, max)}\n…(truncated)` : s)

const system = (widened: boolean): string =>
  [
    'You are the librarian writing the agent-facing summary BODY for a knowledge document (OKF concept body).',
    widened
      ? 'Write: a 2-4 sentence summary of the document’s substance, then a "## Key facts" bullet list carrying every concrete fact, name, number, date, owner, threshold and decision an agent would need without reading the full document — one fact per bullet, and quote figures and identifiers exactly as the document spells them.'
      : 'Write: a 2-4 sentence summary of the document’s substance, then a "## Key facts" bullet list of the concrete facts, names, numbers, and decisions an agent would need without reading the full document.',
    'Summarize the SUBJECT MATTER only — ignore any meta-commentary the document makes about itself (drafting notes, review status, "not yet official", refresh reminders): lifecycle is tracked by the platform, and this summary only exists for PROMOTED documents.',
    'Also propose up to 5 lowercase topic tags on a final line formatted exactly as: TAGS: tag1, tag2.',
    'Factual, terse, no invention. Reply with ONLY the body and the TAGS line.',
  ].join(' ')

// ── The clean step ───────────────────────────────────────────────────────────

/** List bullets and bold markers removed, so `- **TAGS:** a, b` reads the same
 *  as `TAGS: a, b`. The shipped regex was `/^TAGS:\s*(.+)$/m` — anchored, case
 *  sensitive, no decoration allowed — and a small model told to end with a
 *  labelled line writes it as a list item or bolds the label about as often as
 *  it writes it bare. Every one of those was silently zero tags. */
const stripMarkers = (line: string): string => line.replace(/^[\s>*+-]+/, '').replace(/\*\*/g, '').trim()

/** The LAST line that is a TAGS line, with that line removed from the body.
 *
 *  Searched from the end because the prompt asks for it "on a final line": a
 *  document about tagging whose summary legitimately mentions `TAGS:` earlier
 *  should not have its own prose eaten as the contract line. */
function splitTagsLine(raw: string): { body: string; tags: string } {
  const lines = raw.split('\n')
  for (let i = lines.length - 1; i >= 0; i--) {
    const line = lines[i]
    if (line === undefined) continue
    const match = /^tags\s*:\s*(.*)$/i.exec(stripMarkers(line))
    if (!match) continue
    return { body: [...lines.slice(0, i), ...lines.slice(i + 1)].join('\n'), tags: match[1] ?? '' }
  }
  return { body: raw, tags: '' }
}

/** A tag as the OKF frontmatter spells them: lowercase, kebab, nothing else.
 *
 *  The shipped normalizer was `replace(/[^a-z0-9-]/g, '')`, which DELETED the
 *  separator instead of replacing it — "release process" became
 *  "releaseprocess". Mapping runs of non-alphanumerics to a single dash is the
 *  same intent spelled correctly, and it is what makes the kebab assertion in
 *  the evals true by construction rather than by luck. */
const normalizeTag = (raw: string): string =>
  raw
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')

/** A tag long enough to be a sentence is a sentence. The OKF frontmatter is
 *  read by agents choosing which documents to open, and one 200-character
 *  pseudo-tag costs more attention than the other four are worth. */
const MAX_TAG_LENGTH = 40

function parseOkf(raw: string): LibrarianOkf | null {
  const split = splitTagsLine(raw)
  const body = split.body.trim()
  // The one failure this harness has, and it is the shipped one: `if
  // (!text.trim()) return` kept the previous OKF on a model hiccup. Null here
  // means exactly that — kb-okf.ts leaves the doc's existing summary alone
  // rather than replacing it with a heading and nothing under it.
  if (!body) return null

  const tags: string[] = []
  for (const part of split.tags.split(',')) {
    const tag = normalizeTag(part)
    if (!tag || tag.length > MAX_TAG_LENGTH || tags.includes(tag)) continue
    tags.push(tag)
    if (tags.length === 5) break
  }
  return { body, tags }
}

// ── The definition ───────────────────────────────────────────────────────────

export const librarianHarness = defineHarness<LibrarianInput, LibrarianOkf>({
  id: 'librarian',
  label: 'Librarian',
  job: 'Writes each promoted knowledge document’s agent-facing OKF summary — a short digest, the key facts, and topic tags.',

  // Scored by the fitness suite, never blocking: the TAGS line is an
  // instruction-following ask, and a model that fumbles it is worth SEEING in
  // the matrix even though it is not worth refusing over.
  requires: ['instruction-following'],

  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Any model that can write prose can do this job; a weaker one gives thinner key facts and sometimes no tags, and a run it cannot complete leaves the document’s previous summary in place.',
  },

  // Replaces the verbatim chain copy in kb-okf.ts (audit 1.10). Org-scoped: the
  // OKF belongs to the document, not to whoever happened to save it, so there is
  // no userId and no 'preferred' step. No `role: 'utility'` either — the default
  // chain's 'utility' step is the same model under the label the fitness page
  // reads; see the note in titler.ts.
  model: { pin: 'librarian' },

  render: (input, ctx) => [
    { role: 'system', content: system(ctx.widened) },
    { role: 'user', content: `Document "${input.title}":\n\n${clip(input.body, ctx.widened ? WIDE_CLIP : NARROW_CLIP)}` },
  ],

  output: { kind: 'text', clean: parseOkf },

  // FIRE AND FORGET, preserved. Every caller of generateDocOkf is a debounced
  // save or a promotion, and none of them has a human waiting; a failed run must
  // leave the doc's existing OKF alone rather than overwrite it. kb-okf.ts's
  // null check is the other half of this.
  onFailure: 'null',

  // A capable model reads the WHOLE document instead of its first 12,000
  // characters and returns a denser key-facts list. That is more input and more
  // extraction, not more words about the same input — and it expands nothing
  // about what the librarian is allowed to do, which is the line widening must
  // never cross. A model nobody has probed keeps the narrow clip, which works
  // everywhere.
  widen: {
    requires: ['long-context'],
    note: 'A model with a large context window is shown the whole document instead of its first 12,000 characters, and returns a fuller key-facts list.',
  },

  // The OKF is PERSISTED into kb_docs and served to agents through the doc API,
  // so a credential the model copied out of the document body would live there
  // and be read back by every agent that opens the doc. Redaction re-applies the
  // clean step afterwards, so a redacted summary is still a well-formed one.
  //
  // NARROWED by the reconcile pass, and the argument is the summarizer's with
  // teeth: this harness digests knowledge documents, and a knowledge base is
  // full of incident runbooks and postmortems. A faithful key-facts list of
  // "SEV1 is a full outage; page immediately" is `fabricated_outage`'s pattern
  // verbatim, and "tickets are filed automatically" is `zero_tool_claim`'s, with
  // an empty tool record because a librarian turn calls no tools. Those findings
  // land in `guard_findings` under the MODEL's name and are read back as its
  // live confabulation rate next to its benchmark scores, so leaving them on
  // would make every model that summarizes an incident page look like a liar.
  // Nothing is lost that this output could actually do wrong: the credential and
  // the personal detail are what a document body can carry into a persisted
  // summary, and both still run.
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },

  temperature: 0.2,

  evals: [
    {
      name: 'ordinary reference document',
      input: {
        title: 'Release train',
        body: [
          'Talaria ships on a weekly train. The cut happens Thursday 17:00 UTC and the release goes out Friday morning once the smoke suite is green.',
          '',
          'Anything not merged by the cut waits for the next train. Hotfixes are exempt and go out on demand, but they need a second reviewer named in the ticket.',
          '',
          'Ana owns the train rota. The rota is two people: one driver and one backup, rotating fortnightly.',
          '',
          'Rollback target is 15 minutes from the decision to the previous build being live. We have hit that on four of the last five rollbacks; the miss was a database migration that could not be reversed in place, which is why migrations now ship one train ahead of the code that reads them.',
        ].join('\n'),
      },
      check: checkOkf,
    },
    {
      name: 'document that talks about itself',
      input: {
        title: 'Incident severity levels',
        body: [
          'DRAFT — not yet official. Review by the end of the quarter, and refresh this page whenever the on-call rota changes.',
          '',
          'SEV1 is customer-visible data loss or a full outage. Page immediately, no waiting for business hours.',
          'SEV2 is degraded service: a feature is down or slow for a subset of customers. Page during business hours, ticket otherwise.',
          'SEV3 is everything else worth writing down. Ticket only.',
          '',
          'A SEV1 needs a written postmortem within five working days. SEV2 needs one only if it recurs within a month.',
        ].join('\n'),
      },
      // Same contract, plus the instruction the prompt spends a whole sentence
      // on: the lifecycle chatter at the top of the document is the PLATFORM's
      // business and must not end up in the summary agents read.
      check: (value) => checkOkf(value) ?? checkNoMetaCommentary(value),
    },
  ],
})

// ── Eval assertions ──────────────────────────────────────────────────────────
//
// Deterministic string facts, no second model in the loop. Each returns null to
// pass or ONE line naming what was wrong — that line is what an admin reads in
// the fitness drill-down, so it says what the model did, not what an assertion
// was called.

const KEBAB = /^[a-z0-9]+(?:-[a-z0-9]+)*$/
/** A summary longer than this is the model handing the document back. */
const MAX_BODY = 4_000

function checkOkf(value: LibrarianOkf): string | null {
  if (!/^##\s+Key facts\s*$/im.test(value.body)) return 'the body has no "## Key facts" section'
  if (/^\s*(?:[-*+>]\s*)?\**\s*tags\s*:/im.test(value.body)) return 'a TAGS line was left in the body instead of being parsed out'
  if (value.body.length > MAX_BODY) return `the body is ${value.body.length} characters — this is a summary, not a copy of the document`
  if (value.tags.length === 0) return 'no TAGS line was returned, so the document got no topic tags'
  if (value.tags.length > 5) return `${value.tags.length} tags were returned; the contract is at most 5`
  const bad = value.tags.find((t) => !KEBAB.test(t))
  if (bad) return `tag "${bad}" is not lowercase-kebab`
  return null
}

const META = /\b(draft|not yet official|review by|refresh this page|work in progress)\b/i

function checkNoMetaCommentary(value: LibrarianOkf): string | null {
  const hit = META.exec(value.body)
  return hit ? `the summary repeats the document’s own lifecycle commentary ("${hit[0]}") instead of its subject matter` : null
}
