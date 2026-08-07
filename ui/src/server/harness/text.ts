// The one first-line extractor for TEXT harnesses, the way json.ts is the one
// extractor for structured ones.
//
// WHY THIS FILE EXISTS
//   `titler.ts` and `skill-summaries.ts` shipped the same eight lines — first
//   non-empty line, strip leading `["'#*\s]`, strip trailing quotes — written
//   twice with a one-character difference (the titler stripped a trailing `*`,
//   the summarizer did not). Two copies of one contract is how the audit's six
//   JSON extractors happened, and the port arrived with both copies intact.
//
//   Executing them against realistic 7-14B replies found two shapes that store
//   GARBAGE rather than failing, which is the worse of the two failure modes:
//   a failure keeps the previous title/summary, but a stored "```" persists
//   until the content hash changes.
//
//     "```\nWrites release notes.\n```"      stored the literal "```"
//     "**Writes release notes** from PRs."   stored a trailing "**" (summarizer)
//     "Here's the summary:\n\nWrites …"      stored the lead-in
//
//   All three are the model wrapping its answer rather than refusing to answer,
//   so the right response is to unwrap it, not to fail the contract and leave a
//   chat wearing its first message. Fixed once, here, and both harnesses call
//   it — which is the whole point of having one copy.
//
// PURE. No database, no gateway, no settings: a harness definition importing
// this stays enumerable by the fitness suite without booting Talaria.

/** A line that is nothing but a code-fence delimiter (``` or ```bash). It is
 *  never the answer, and it was being stored AS the answer. */
const FENCE_ONLY = /^\s*`{3,}[\w-]*\s*$/

/** A lead-in the model wrote before its real answer ("Here's the summary:").
 *  Only skipped when something follows it — a one-line reply that happens to
 *  end in a colon is that reply, and dropping it would fail a usable value. */
const LEAD_IN = /:\s*$/

/** The same thing WITHOUT the colon, which is how a small model most often
 *  ignores "reply with ONLY the title": "Sure, here's a good title\n\nCheckout
 *  Latency on Mobile". The colon form was skipped and this one was STORED — as
 *  a conversation's name, as a skill's Studio subtitle — and because the stored
 *  preamble no longer matches the mechanical title, neither retitle sweep ever
 *  revisits the row. Worse for the fitness suite: a 3-7 word apology passes
 *  every assertion `titleProblem` makes, so a model that prefaces every reply
 *  scored GREEN on the one job whose whole contract is instruction-following.
 *
 *  ANCHORED OPENERS ONLY, and only when an answer follows. This is deliberately
 *  a list of ways a model addresses the USER rather than a guess at what a title
 *  looks like: "Based on the transcript" is never a title, but "Checkout latency
 *  on mobile" must survive untouched, and a heuristic that reaches for meaning
 *  would eventually eat one. */
const CONVERSATIONAL_LEAD_IN =
  /^(?:sure|certainly|absolutely|of course|ok|okay|got it|understood|no problem|here (?:is|are)|here's|below (?:is|are)|based on|as requested|i(?:'ll| will| have| can)\b)/i

/** A list marker the model put in front of a one-line answer. `*` is already in
 *  `DECORATION_LEADING`; `-`, `+`, `•` and `1.` / `1)` were not, so a model that
 *  had been reading markdown all prompt named a chat "- Checkout latency on
 *  mobile". The trailing space is required so a negative number or an en-dash
 *  title is not a list. */
const LIST_MARKER = /^(?:[-+•*]|\d{1,3}[.)])\s+/

/** Balanced inline markup around part of the line. Unwrapped BEFORE the
 *  edge-decoration strip below, because that strip only fires at the edges: on
 *  "**Checkout Latency** on Mobile" it removed the opening `**` and left the
 *  closing one mid-string, turning a symmetric shape anything could strip into
 *  an asymmetric one nothing can. That partial-bold case is one of the three
 *  this file's header says it was created to fix. */
const INLINE_MARKUP = /(\*\*|__|`)(.+?)\1/g

/** Decoration a model wraps a one-line answer in. Leading `#` is a heading
 *  marker; it is deliberately NOT in the trailing class, because `#` at the end
 *  of a line is content ("Sprint 14 planning #3") far more often than it is a
 *  closing ATX marker. */
const DECORATION_LEADING = /^["'“‘#*\s]+/
const DECORATION_TRAILING = /["'”’*\s]+$/

/** The first line of a reply that is actually the answer, undecorated — or null
 *  when the reply carried nothing usable, which every caller reads as "keep
 *  what you had". */
export function firstMeaningfulLine(raw: string): string | null {
  const lines = raw.split('\n').filter((l) => !FENCE_ONLY.test(l))
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]?.trim()
    if (!line) continue
    // A lead-in only counts as one if there is an answer under it.
    const answerFollows = lines.slice(i + 1).some((l) => l.trim())
    if (answerFollows && (LEAD_IN.test(line) || CONVERSATIONAL_LEAD_IN.test(line))) continue
    const stripped = line
      .replace(LIST_MARKER, '')
      .replace(INLINE_MARKUP, '$2')
      .replace(DECORATION_LEADING, '')
      .replace(DECORATION_TRAILING, '')
    if (stripped) return stripped
  }
  return null
}
