// The Titler, declared. Names things as they take shape: chats and plans after
// their first exchange, research runs from their question.
//
// This is the smallest harness in the product and it is the one that proves the
// contract, because everything it used to do by hand is now somebody else's
// problem: the eight-line fallback chain (audit 1.10 — hand-copied verbatim into
// four files, 'pl-main' spelled out as a literal in seven), the try/catch that
// turned an upstream hiccup into a silent null, and the metering it never did at
// all. What is left below is the three prompts, the output contract, and the
// floor — which is the entire point of the port.
//
// THE FLOOR IS EMPTY ON PURPOSE. Naming a chat is the most forgiving job in
// Talaria: the reply is one short line, nothing downstream parses it, and a
// mediocre title is strictly better than the mechanical first-message
// truncation it replaces. A titler that REFUSED to run on an unprobed 7B model
// would leave every conversation wearing its truncation forever, which is a
// worse product than a slightly clumsy name. `requires` still says what the job
// actually leans on — following "reply with ONLY the title" — so the fitness
// suite can score it and an admin can see the weakness, but nothing here blocks.
import { defineHarness, type EvalCase, type Message } from '../define'
import { firstMeaningfulLine } from '../text'

export type TitleKind = 'chat' | 'plan' | 'research'

export interface TitlerInput {
  kind: TitleKind
  /** A transcript for chat/plan, the question itself for research. */
  text: string
}

// Verbatim from the hand-written titler: three jobs that share a shape and not a
// wording. A chat is named for its subject, a plan for what it delivers, a
// research run for what it investigates — and the research prompt has to say
// "not a question" out loud, because the input IS a question and a small model
// will otherwise hand it straight back.
const PROMPT: Record<TitleKind, string> = {
  chat:
    'Name this conversation. 3–7 words, specific to what it is actually about — the subject, not the activity. ' +
    'No quotes, no trailing punctuation, never generic fillers like "Chat about" or "Discussion of". Reply with ONLY the title.',
  plan:
    'Name this plan. 3–7 words, outcome-focused — what the plan will deliver, not the conversation around it. ' +
    'No quotes, no trailing punctuation. Reply with ONLY the title.',
  research:
    'Name this research run from its question. 3–7 words capturing the subject under investigation. ' +
    'No quotes, no trailing punctuation, do not restate it as a question. Reply with ONLY the title.',
}

/** Naming is worth one short call and no more. A transcript longer than this
 *  says nothing about its own subject that the first few thousand characters
 *  did not, and paying to send it would make the cheapest harness in the
 *  product the most expensive one on a long chat. */
const clip = (s: string, max = 4000): string => (s.length > max ? s.slice(0, max) : s)

/** Raw reply -> a title, or null to fail the contract (the caller then keeps
 *  the title it already had).
 *
 *  The unwrapping half is `firstMeaningfulLine` (harness/text.ts) — the shared
 *  text-harness extractor, which is where this file's hand-written copy went
 *  when the reconcile pass found the summarizer carrying the same eight lines
 *  with a one-character difference. What stays here is what is TRUE OF TITLES
 *  and of nothing else:
 *    - the trailing period, including the ideographic '。': the prompt says no
 *      trailing punctuation and models add one anyway.
 *    - the 90-character clamp with an ellipsis: a title is rendered in a sidebar
 *      row, and a model that ignored "3-7 words" must not be able to push a
 *      paragraph into that row.
 *  The one thing it does NOT do is reject a long or generic title — that is the
 *  fitness suite's job (see `evals`), not a reason to leave a chat unnamed. */
export function cleanTitle(raw: string): string | null {
  const line = firstMeaningfulLine(raw)
  if (line === null) return null
  const t = line.replace(/[.。]$/, '').trim()
  if (!t) return null
  return t.length > 90 ? `${t.slice(0, 90).trimEnd()}…` : t
}

// ── Eval assertions ──────────────────────────────────────────────────────────
// Deterministic string facts, no judge model. These are the titler's row in the
// fitness matrix, and they are the reason a candidate model can be rejected for
// this job in seconds rather than after a week of oddly-named conversations.

const words = (s: string): string[] => s.split(/\s+/).filter(Boolean)
const norm = (s: string): string =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, ' ')
    .trim()

/** The fillers the chat prompt names, plus their obvious siblings. Kept to
 *  CONVERSATION nouns: "Analysis of index bloat" is a perfectly good title and a
 *  wider list would fail a model for writing one. */
const GENERIC_LEAD_IN =
  /^(?:(?:a|an|the)\s+)?(?:chat|conversation|discussion|dialogue|thread|talk|exchange|session|notes?)\s+(?:about|on|of|regarding|concerning|with|between|for)\b/i

/** Did the model just hand the input back? Compared line by line with the
 *  speaker prefix removed, because a chat transcript arrives as "user: ..." and
 *  a restatement of the first message is the classic small-model answer. Short
 *  titles are exempt: a three-word title that happens to open a sentence is a
 *  coincidence, not a restatement. */
function restatesInput(title: string, text: string): boolean {
  const t = norm(title)
  if (t.length < 12) return false
  return text
    .split('\n')
    .map((line) => norm(line.replace(/^\s*(?:user|assistant|system)\s*:\s*/i, '')))
    .some((line) => line.startsWith(t))
}

/** One line naming what is wrong with this title, or null. This is what an
 *  admin reads in the fitness drill-down, so it says the observed fact rather
 *  than the rule id. */
export function titleProblem(title: string, input: TitlerInput): string | null {
  if (title.length > 90) return `${title.length} characters — a title has to fit one sidebar row`
  const n = words(title).length
  if (n < 3 || n > 7) return `${n} word${n === 1 ? '' : 's'} — the prompt asks for 3-7`
  if (/^["'“‘]|["'”’]$/.test(title)) return 'wrapped in quotes'
  if (/[.。!?]$/.test(title)) return 'ends in punctuation'
  if (GENERIC_LEAD_IN.test(title)) return 'opens with a generic filler ("Chat about", "Discussion of")'
  if (input.kind === 'research' && /\?/.test(title)) return 'restated the question instead of naming the subject'
  if (restatesInput(title, input.text)) return 'restates the input verbatim instead of naming it'
  return null
}

/** One fixture. The check closes over the SAME input the model was given, so
 *  the restatement assertion is measured against the real transcript rather
 *  than a copy that can drift away from it. */
const titleCase = (name: string, band: EvalCase<TitlerInput, string>['band'], input: TitlerInput): EvalCase<TitlerInput, string> => ({
  name,
  band,
  input,
  check: (value) => titleProblem(value, input),
})

export const titlerHarness = defineHarness<TitlerInput, string>({
  id: 'titler',
  label: 'Titler',
  job: 'Names things as they take shape: chats and plans after their first exchange, research runs from their question.',
  // Not a floor — a fact for the fitness matrix. "Reply with ONLY the title" is
  // the whole contract, and a model that answers it with a paragraph is a model
  // whose titles get clamped to 90 characters of preamble. Worth measuring,
  // never worth refusing over.
  requires: ['instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Runs on any model. A weak one writes clumsier names; nothing downstream reads a title, and a clumsy name beats leaving every chat wearing its first message.',
  },
  // The chain that was hand-written here: the Titler pin from Models → Platform,
  // then the Utility role, then the env default, then the first routable model.
  // Same order, one implementation (harness/model.ts) — and no `role` field,
  // because the DEFAULT chain already carries a 'utility' step. Declaring
  // `role: 'utility'` as well would win one step earlier and record
  // `harness_runs.chain_step = 'role'` for the same resolved model, so two
  // harnesses resolving identically would report different steps to the fitness
  // page. 'utility' means "the Utility role model carried this" everywhere;
  // 'role' is reserved for a harness that has a role of its own.
  model: { pin: 'titler' },
  render: (input): Message[] => [
    { role: 'system', content: PROMPT[input.kind] },
    { role: 'user', content: clip(input.text) },
  ],
  output: { kind: 'text', clean: cleanTitle },
  // Fire and forget, and that is a product decision rather than laziness: every
  // caller of this harness already HAS a title (the mechanical truncation, or
  // nothing at all for a research run) and null means "keep it". sweepTitles
  // additionally reads null as a stop signal so a dead model cannot burn a whole
  // batch. Anything other than 'null' here would break both.
  onFailure: 'null',
  // Only the two rules that can fire on a five-word title, and the omission
  // matters more than the inclusion: `zero_tool_claim` reads "Deleted the stale
  // billing rows" as a claim of completed work with no tool behind it, which for
  // a TITLE is a false positive by construction — and guard_findings per model
  // is the live confabulation rate the fitness page shows, so a titler filing a
  // finding on every past-tense name would libel the model it runs on.
  // Titles are persisted and rendered in a sidebar, so a credential that reached
  // one gets scrubbed before it is stored.
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  temperature: 0.3,
  // No `widen`. A frontier model already writes a better title from this exact
  // prompt; there is no MORE for it to do here, and the only thing a wider
  // prompt would buy is a longer name — which is the failure mode, not the
  // upgrade. Widening exists for depth and authority, and a title has neither.
  //
  // TEN FIXTURES ACROSS THREE BANDS. EASY is one obvious subject stated plainly
  // — a model that cannot name that cannot title anything. STANDARD is the job
  // as it arrives: a real transcript with more than one noun in it. HARD is
  // where the transcript actively misleads — a loud opening line that is not the
  // subject, a quotable phrase that is a trap, a question short enough that
  // handing it back looks like an answer.
  evals: [
    titleCase('chat — one plain subject, stated outright', 'easy', {
      kind: 'chat',
      text: [
        'user: the nightly backup job has been failing since Tuesday',
        'assistant: the target volume filled up — the retention sweep stopped running when the cron user lost write access.',
      ].join('\n'),
    }),
    titleCase('research — one plain subject, stated outright', 'easy', {
      kind: 'research',
      text: 'How do European data residency rules apply to customer support transcripts?',
    }),
    titleCase('plan — one plain deliverable, stated outright', 'easy', {
      kind: 'plan',
      text: [
        'user: we need SSO working for the enterprise trial next month',
        'assistant: that is SAML for two identity providers plus a group-to-role mapping.',
      ].join('\n'),
    }),
    titleCase('chat — names the subject, not the activity', 'standard', {
      kind: 'chat',
      text: [
        'user: our checkout page takes about nine seconds to load on mobile and people are dropping off at payment',
        'assistant: the largest contentful paint is dominated by the payment iframe — it blocks render until the provider script resolves.',
      ].join('\n'),
    }),
    titleCase('chat — does not adopt a quoted phrase from the transcript', 'standard', {
      kind: 'chat',
      text: [
        'user: someone filed a ticket called "URGENT!!! everything is broken." can we work out what they actually mean',
        'assistant: the attached log shows a single failing migration on the reporting replica.',
      ].join('\n'),
    }),
    titleCase('plan — names the outcome, not the conversation', 'standard', {
      kind: 'plan',
      text: [
        'user: we need to get the warehouse off the old label printer before the holiday rush',
        'assistant: that means new firmware on twelve stations, a template migration, and a fallback for the two sites still on serial.',
      ].join('\n'),
    }),
    titleCase('research — names the subject without restating the question', 'standard', {
      kind: 'research',
      text: 'What are the practical tradeoffs between Postgres logical replication and Debezium for feeding a warehouse in near real time?',
    }),
    // THE BURIED SUBJECT. The loudest line is the opening complaint and it is
    // not what the conversation turns out to be about. A weaker model titles the
    // first sentence it read.
    titleCase('chat — the subject is not the opening line', 'hard', {
      kind: 'chat',
      text: [
        'user: I am so tired of this deploy pipeline, it is genuinely the worst part of my week, every single time',
        'assistant: which step is failing for you?',
        'user: honestly the pipeline is fine. what keeps biting me is that staging and production have different Postgres extensions installed, so migrations pass in one and fail in the other.',
        'assistant: so the real problem is extension drift between environments.',
      ].join('\n'),
    }),
    // A QUESTION SHORT ENOUGH TO HAND BACK. The prompt says "do not restate it
    // as a question"; the cheap move is to strip the question mark and return
    // the same words, which `restatesInput` catches.
    titleCase('research — a short question it must not simply hand back', 'hard', {
      kind: 'research',
      text: 'Is Redis Streams a good fit for our job queue?',
    }),
    // A CONVERSATION ABOUT CONVERSATIONS. Every generic filler the prompt bans
    // is sitting right there in the transcript for the taking.
    titleCase('chat — a meta subject, with every filler word available to steal', 'hard', {
      kind: 'chat',
      text: [
        'user: we keep having the same discussion about how we run these meetings and nothing changes',
        'assistant: what usually derails it?',
        'user: nobody writes down the decision, so the next meeting relitigates it. we need a decision log.',
      ].join('\n'),
    }),
  ],
})
