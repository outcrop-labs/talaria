// The drafting muse: users iteratively construct agent internals (souls,
// personalities, skills, memories, cron jobs) with an LLM. This file owns the
// PROMPTS and the model policy; how a given kind reaches a model is split in
// two, and the split is the point:
//
//   six PROSE kinds stream, through `runHarnessStreamed` + `gatewayStream`.
//   Live tokens landing in the editor is the feature, so the runner pumps the
//   deltas out as they land and does everything it always does with the
//   completed reply — the guard pass, the findings row, the harness_runs row
//   (`muse:draft` in harness/defs/muse.ts). Strict guard mode additionally
//   redacts chunk-wise on the way OUT, in the route, because on this path the
//   streamed copy IS the copy the user saves and only the caller is on that
//   path. There is no replay transport and no second SSE pump any more; prompts
//   and model policy still live in this file and the definition renders through
//   `buildMuseMessages` below, so there is exactly one spelling of the ask.
//
//   three JSON kinds (cron, agent, ticket) go through `runHarness` — see
//   harness/defs/muse.ts. They used to stream too, and the browser parsed the
//   result with a greedy `/\{[\s\S]*\}/` (audit 1.1) in the one place where no
//   repair turn, no guardrail and no telemetry are possible. They are parsed and
//   validated on this side now, and the route returns the value.
import { orgLine, orgProfile } from './org'
import { resolveHarnessModel, type ModelSpec } from './harness/model'
import type { Message } from './harness/define'

export type MuseKind = 'soul' | 'personality' | 'skill' | 'memory' | 'cron' | 'agent' | 'document' | 'template' | 'ticket'

/** The kinds whose answer is a CONTRACT rather than a document. Exported so the
 *  route has one place to branch on, instead of three string comparisons that
 *  can drift from the three harness definitions. */
export type MuseJsonKind = 'cron' | 'agent' | 'ticket'
export const JSON_KINDS: ReadonlySet<MuseKind> = new Set<MuseKind>(['cron', 'agent', 'ticket'])
/** Narrows, so the route can index a per-kind message table without a cast. */
export const isJsonKind = (k: MuseKind): k is MuseJsonKind => JSON_KINDS.has(k)

/** THE SHAPE OF A SOUL.md, STATED ONCE.
 *
 *  IT USED TO BE STATED ONLY FOR `agent`, and `soul` — the kind whose entire job
 *  is writing one — never got it. `SYSTEM.soul` said "keep the heading
 *  structure", which is an instruction about a document that already exists; the
 *  fixture that grades a soul written FROM SCRATCH requires it to open with a
 *  `# <Name> — <Role>` title, and nothing in the prompt had ever asked for one.
 *  The only heading named anywhere in the assembled prompt was `## Who you are`,
 *  from the organization anchor — so models opened there, correctly following
 *  the only structural instruction they were given, and were failed for it.
 *
 *  Shared by both kinds so they cannot drift: a soul written by the agent
 *  designer and a soul written by the soul editor are the same document. */
const SOUL_SHAPE =
  'a "# <Name> — <Role>" title as the very first line, then "## Who you are" (identity + mission), ' +
  '"## Voice & personality" (a distinct, likable working voice), and "## How you work".'

const DOC_RULES =
  'Return ONLY the complete revised document — no commentary, no preamble, no code fences. ' +
  'Start from the current version when one is given: keep what works, change what the request asks, never silently drop sections.'

const SYSTEM: Record<MuseKind, string> = {
  soul:
    'You write SOUL.md files for Hermes agents — the markdown document that defines who an agent is: identity, personality, operating principles, and guardrails. ' +
    `Writing one from scratch, its shape is ${SOUL_SHAPE} ` +
    'Revising one, keep the heading structure it already has, keep it tight and actionable, and preserve existing guardrails unless explicitly asked to change them. ' +
    DOC_RULES,
  personality:
    "You write the personality brief for someone's personal AI assistant: how it should come across — tone, priorities, pet peeves. " +
    'Plain prose, second person ("Be"), a few sentences to a short paragraph; no headings. ' +
    DOC_RULES,
  skill:
    'You write SKILL.md playbooks that an AI agent follows for a recurring job. Markdown with a # title, a line on when to use it, and concrete numbered steps; ' +
    'be specific about tools/sources when the request names them. ' +
    DOC_RULES,
  memory:
    "You curate an AI agent's MEMORY.md — durable facts, preferences, and context it should remember. Terse bullet lines, grouped under short headings when helpful; " +
    'never invent facts — only reorganize, prune, or add what the request states. ' +
    DOC_RULES,
  cron:
    'You design a scheduled job for an AI agent. Reply with ONLY a JSON object — no fences, no commentary — shaped exactly: ' +
    '{"name": "<kebab-case-short-name>", "schedule": "<5-field cron expr or interval like \\"every 2h\\"/\\"30m\\">", "prompt": "<self-contained instruction the agent executes each run>"} ' +
    'Times are UTC. Prefer cron expressions for fixed times ("0 14 * * 1-5"), intervals for frequencies.',
  agent:
    'You design a complete Hermes AI agent from a purpose description. Reply with ONLY a JSON object — no fences, no commentary — shaped exactly:\n' +
    '{"name": "<short human first-name, e.g. \\"Rex\\">", "handle": "<lowercase alphanumeric, starts with a letter, 2-30 chars>", ' +
    '"department": "<lowercase-kebab function word, e.g. \\"release\\" or \\"research\\">", "role": "<human job title, e.g. \\"Release Manager\\">", ' +
    '"soul": "<the full SOUL.md markdown>", "skills": [{"name": "<kebab-case>", "content": "<full SKILL.md markdown>"}]}\n' +
    `The soul is the agent's defining document: ${SOUL_SHAPE} "## How you work" MUST keep these guardrails: ` +
    'keep humans in the loop (create and triage tickets, never assign or close them); prefer the local model tier for routine work, escalate deliberately; ask in the channel instead of guessing. ' +
    'Include 0–3 skills, only ones clearly implied by the purpose (each a # title, a when-to-use line, concrete numbered steps). ' +
    'When a current draft is given, revise it per the request instead of starting over — keep everything not asked about.',
  document: 'You help edit a markdown document. ' + DOC_RULES,
  // THE CLOSED WORLD IS STATED THREE TIMES, and that is deliberate rather than
  // sloppy. This prompt has exactly two jobs — patch a field, or refuse — and a
  // small model asked to "assign this to Dana and move it to the design board"
  // used to do neither: it reached for the nearest fields it COULD write and
  // answered `{"status":"in_progress","tags":["design"]}`, which is a
  // plausible-looking patch of something nobody asked for, sitting behind an
  // Apply button. It is the worst failure this surface has, because a refusal
  // costs a sentence and a wrong patch costs a board.
  //
  // The old wording carried the escape hatch as the LAST clause of a run-on
  // "Rules:" paragraph, after a 90-word field list and a strong "you make edits"
  // framing. So: the boundary is named before the fields, the fields are named
  // as the whole of the world, the out-of-scope asks a user actually types are
  // listed as CONCRETE NOUNS (a small model matches "assignee" far more reliably
  // than it infers the complement of a set), and the substitution failure is
  // forbidden by name with the refusal shown as a worked example.
  ticket:
    'You make fast edits to a project TICKET from a natural-language instruction. The current ticket is given as JSON.\n' +
    'You can change TEN fields and nothing else. Anything else the instruction asks for, you refuse — you never approximate it with a field you do have.\n' +
    'Reply with ONLY a JSON object — no prose, no code fence — containing exactly the fields to CHANGE:\n' +
    '{ "title"?: string, "description"?: markdown string (the FULL replacement), "priority"?: "low"|"medium"|"high"|"urgent", ' +
    '"effort"?: "xs"|"s"|"m"|"l"|"xl"|null, "estimatedHours"?: number|null, "dueDate"?: ISO datetime|null, "startDate"?: ISO datetime|null, ' +
    '"color"?: "slate"|"bronze"|"green"|"amber"|"red"|"blue"|"purple"|"teal"|"pink"|"orange"|"lime"|"cyan"|"indigo"|"magenta"|"olive"|"brown"|null, ' +
    '"tags"?: string[] (the FULL replacement label set), "status"?: "inbox"|"assigned"|"in_progress"|"blocked"|"quality_review"|"done" }\n' +
    'THINGS THAT ARE NOT ON THAT LIST, and are therefore always a refusal: the assignee or owner, the board or project the ticket lives on, ' +
    'comments, attachments, subtasks, linked or blocking tickets, watchers, sprints, estimates of anyone else\'s time, and deleting or archiving the ticket. ' +
    'Refuse by replying exactly {"error": "<one short sentence why>"} and nothing else — for example, ' +
    '{"error": "I can only edit this ticket\'s own fields, not its assignee or board."} ' +
    'A refusal is the RIGHT answer here and costs nothing; a patch of a different field is silently wrong and a person has to undo it.\n' +
    'Rules: include ONLY fields the instruction asks to change; omit everything else — an instruction to change one field never changes a second. ' +
    'Relative dates resolve against the "now" timestamp in the context. ' +
    'Rewriting or extending the description: return the complete new markdown in "description", preserving everything not asked about. ' +
    'If part of the instruction is in scope and part is not, refuse the whole thing and say which part you cannot do. ' +
    'If the instruction is unclear, refuse the same way.',
  template:
    'You write TEMPLATES for Talaria — the markdown skeleton a ticket description or plan document STARTS from. A template is scaffolding, never a finished document.\n' +
    'Hard rules:\n' +
    '- ## section headings only (no #, no ###). 3–6 sections; more than 6 means you are overbuilding.\n' +
    '- Under each heading: NOTHING, or a single italic placeholder hint (_one line describing what goes here_), or 2–4 empty bullet stubs ("- "). Never real content, never example prose, never filled-in details.\n' +
    '- Whole template under 25 lines. If the request describes a big process, capture it as section NAMES, not content.\n' +
    '- If the request asks for a complete document, an essay, or filled-in content, still return only the skeleton such a document would start from.\n' +
    'Shape example (do not copy the topic):\n## Summary\n_What and why, in two sentences._\n## Steps\n- \n- \n## Out of scope\n_What this deliberately does not cover._\n' +
    'When revising an existing template: prune verbosity first — tighten hints, merge overlapping sections; never grow it past the rules above. ' +
    DOC_RULES,
}

export interface MuseInput {
  kind: MuseKind
  instruction: string
  /** The document as it stands in the editor (context + revision base). */
  current?: string
  /** One line of situational context, e.g. the agent's name and role. */
  context?: string
  /** Prior muse turns in this session, for iterative refinement. */
  chat?: Array<{ role: 'user' | 'assistant'; content: string }>
}

/** Kinds that define WHO an agent is — these anchor to the organization. */
const ORG_KINDS = new Set<MuseKind>(['agent', 'soul', 'personality'])

/** The one prompt difference a capability-gated widening buys (see the argument
 *  on `museAgentHarness.widen`). Same number of skills either way, and the same
 *  guardrails: what changes is how much document the model is asked to hold in
 *  one nested JSON string, which is the thing that actually breaks on a 7B. */
const AGENT_SKILL_DEPTH: Record<'wide' | 'narrow', string> = {
  wide: '\n\nWrite each skill out in full: the real steps, the tools or sources it touches, and what "done" looks like.',
  narrow: '\n\nKeep each skill under 25 lines. A short playbook that is complete beats a long one that gets cut off mid-sentence.',
}

/** `Message[]`, not the old `Array<{role: string; content: string}>` — a
 *  harness `render` must return the union the transports actually accept, and
 *  the roles here were already exactly those three. */
export async function buildMuseMessages(input: MuseInput, opts?: { widened?: boolean }): Promise<Message[]> {
  let system = SYSTEM[input.kind]
  if (input.kind === 'agent') system += AGENT_SKILL_DEPTH[opts?.widened ? 'wide' : 'narrow']
  if (ORG_KINDS.has(input.kind)) {
    // The org anchor is DECORATION on the prompt, so a settings read that fails
    // must not take the draft down with it. Inside a harness `render`, a throw
    // surfaces as the unhelpful "rendered no messages" — an agent design lost to
    // a blip on a table that only supplies one sentence of context.
    const org = await orgProfile()
      .then(orgLine)
      .catch(() => null)
    if (org) {
      // WHERE TO ANCHOR THE IDENTITY IS KIND-SPECIFIC, and naming a heading here
      // unconditionally contradicted one of the three prompts it is appended to.
      //
      // WHAT IT COST. `SYSTEM.personality` asks for "Plain prose ... no
      // headings". This clause then told the same model to name the business in
      // `## Who you are`. Every model resolved the contradiction the same way —
      // by writing the heading — and four of them, across two fixtures, opened
      // with the identical string `## Who you are`: gemma-4-31b, gemma-4-26b,
      // haiku-4.5 and muse-glimmer. The suite recorded four model failures for
      // following the more specific of two instructions we sent.
      //
      // It also broke the SOUL fixture beside it, which requires the reply to
      // OPEN with its `# <Name> — <Role>` title: a model told to name the
      // business in `## Who you are` starts there instead.
      //
      // So the anchor names a place only where a place exists. `personality` is
      // a paragraph and gets the instruction without the heading.
      const anchor = input.kind === 'personality' ? 'anchor its voice and priorities to the business' : 'anchor its identity, mission, and voice to the business (name it in "## Who you are")'
      system +=
        `\n\nOrganization: ${org}. The agent is a member of this business's team — ${anchor}; ` +
        `it never presents itself as belonging to an underlying platform, framework, or model vendor.`
    }
  }
  if (input.context) system += `\n\nContext: ${input.context}`
  if (input.current?.trim()) system += `\n\nCurrent version:\n<<<\n${input.current}\n>>>`
  else system += '\n\nThere is no current version yet — write one from scratch.'
  return [
    { role: 'system', content: system },
    ...(input.chat ?? []).slice(-12),
    { role: 'user', content: input.instruction },
  ]
}

/** The Muse's model policy, declared once instead of written out (audit 1.10 —
 *  this was one of the seven hand-copied spellings of the fallback chain, and
 *  1.7, which is why 'pl-main' no longer appears in this file at all).
 *
 *  The ORDER is today's, preserved exactly, and it is not the default chain:
 *
 *    pin        an admin-assigned Muse model (Models → Platform) is ORG POLICY
 *               and wins over a personal preference.
 *    preferred  then the user's own choice — the whole reason the Muse is
 *               user-scoped and the reason 'preferred' comes before the role.
 *    utility    then the org's Utility role model. Spelled as the 'utility'
 *               step rather than `role: 'utility'` + a 'role' step: the two
 *               resolve identically (harness/model.ts runs `resolveRoleModel`
 *               under the same allowlist gate either way) and differ only in the
 *               `chain_step` recorded on the run, so every harness in the tree
 *               uses the one label. Same chain as the distiller and the
 *               concluder, which are the other two user-scoped harnesses.
 *    env        then TALARIA_COPILOT_MODEL.
 *    first-…    then whatever the gateway serves (which prefers 'pl-main' where
 *               that name exists, so the reference deployment resolves as it
 *               always did, and a self-host that never used the name still gets
 *               a real model instead of nothing).
 *
 *  THE MEMBER MODEL ALLOWLIST SURVIVES THIS. It is an admin gating the expensive
 *  brains, and `resolveHarnessModelWith` applies it to exactly the steps that
 *  hand a user's own choice or the user-visible catalog to a harness —
 *  'preferred', 'role', 'first-routable' — and deliberately not to 'pin' or
 *  'env', which is the same distinction this function drew by hand. It is armed
 *  by `userId` being present on the spec, so every caller must pass one. */
export const MUSE_MODEL: ModelSpec = { pin: 'muse', chain: ['pin', 'preferred', 'utility', 'env', 'first-routable'] }

/** The model a user's muse resolves to right now. Kept as a function because
 *  two other subsystems ask the question without running a harness: /api/models
 *  shows it in the picker, and comms-decay falls back to it for the distiller
 *  and the concluder. */
export async function museModelFor(userId: string): Promise<string | null> {
  const resolved = await resolveHarnessModel({ ...MUSE_MODEL, userId })
  return resolved?.model ?? null
}
