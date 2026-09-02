#!/usr/bin/env node
// Talaria invariant check — the tripwire under the predicates that keep getting
// re-implemented. Zero dependencies, plain Node, run it with `node
// scripts/check-invariants.mjs` from anywhere.
//
// WHY THIS FILE EXISTS
//   The recurring shape of bug here: a rule that decides who may read a thing,
//   or which statuses are terminal, gets written out by hand at a new call
//   site instead of imported — merged by a human who had read the code around
//   it, failing nothing. Centralizing a predicate without a check that FAILS
//   on the next copy just schedules the next round. This is that check.
//
//   The Rust port moved the engines — tasks, boards, statuses, approvals,
//   notifications — into the api crate, and the rules whose subjects went with
//   them went too. What remains polices the TypeScript that is still live
//   (the SPA, the resident server tier, mcp/, cli/) and follows two subjects
//   across the language line (upload bytes, the fitness toolbox), because a
//   guard that stops at the edge of the tree its subject moved out of guards
//   nothing.
//
//   One lesson earned twice, kept as machinery: a duplicate-detection rule
//   whose subject is renamed keeps passing on a tree that no longer contains
//   it. A rule that names its subject's home carries a companion that fails
//   when the home goes empty — the allowlist and anchor checks below.
//
// WHY NOT ESLINT
//   A linter is a dependency, a config, a plugin API and a rollout against an
//   unlinted codebase (hundreds of pre-existing failures, then someone disables
//   it). This is ~250 lines of readable regex with the fix written into every
//   failure message. When the repo grows a linter, port these rules to it and
//   delete this file — the rules, not the mechanism, are the valuable part.
//
// HOW TO READ A FAILURE
//   Every rule prints WHAT matched, WHERE, and WHAT TO WRITE INSTEAD. If the
//   suggested fix is wrong for your case, that is a conversation to have in the
//   PR — not a reason to add yourself to a census below.
//
// HOW TO ADD A RULE
//   Push onto RULES (a forbidden pattern) or CENSUS (a pattern that is legal in
//   named places and forbidden everywhere else). Both take a `fix` string; write
//   the instruction, not the observation.
//
// KNOWN LIMIT
//   Comments are stripped line-wise: a line whose first non-space characters are
//   `//`, `/*` or `*` is ignored. A pattern hidden in a TRAILING comment on a
//   line of code still matches. That is deliberate — it keeps this file a thing
//   a human can read and amend, and a false positive here costs one comment
//   rewrite while a false negative costs another round.

import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(fileURLToPath(new URL('.', import.meta.url)), '..')

/** Trees that hold hand-written source. Everything else (generated files, build
 *  output, deps) is skipped by SKIP below. cli/src is the `talaria` CLI — its
 *  commands carry the path/guard predicates (canonical roots, 0600 writes,
 *  destructive-reset gates) that are exactly this file's bug class. */
const SOURCE_DIRS = ['ui/src', 'mcp/src', 'cli/src']
const SKIP_DIRS = new Set(['node_modules', 'dist', 'build', '.output', '.vinxi', '.tanstack'])
const SKIP_FILES = new Set(['routeTree.gen.ts'])
// `.svelte` IS hand-written source, and leaving it out was not a small gap.
// The React→Svelte migration moved the bulk of the UI into 384 .svelte files,
// every rule below silently stopped covering all of them, and the census went
// on naming .tsx paths that no longer existed — so the file both under-checked
// the tree and reported a stale over-count, at the same time, for the same
// reason. A rule that cannot see the majority of the code it polices is
// decoration; this is the line that keeps it a check.
const EXTS = ['.ts', '.tsx', '.mts', '.js', '.mjs', '.svelte']

// ── Rules ────────────────────────────────────────────────────────────────────

/** Forbidden outright, except in the files that legitimately define the thing. */
const RULES = [
  {
    id: 'queryfn-swallows-errors',
    // Scanned structurally, not by regex — see scanQueryFns().
    scan: scanQueryFns,
    what: 'a queryFn that resolves successfully when the request failed',
    fix: [
      'Let it throw. react-query already has an error state; returning [] / null / {} on a',
      'failed fetch reports SUCCESS carrying emptiness, and every consumer downstream renders',
      '"nothing here" for what is actually "we could not ask". `getList`/`getJson` in',
      '@/lib/fetch-json throw with the server message already.',
      'If a specific status genuinely means "none" (a 404 on an optional resource), test THAT',
      'status explicitly and rethrow everything else.',
    ],
  },
  {
    id: 'query-result-discarded-at-the-call-site',
    // Scanned structurally — see scanDiscardedQueries().
    scan: scanDiscardedQueries,
    what: 'a hook result flattened to a default on the line that created it',
    fix: [
      'Use `listQuery()` from @/components/ui/query-state — it returns `{ rows, notice, failed,',
      'stale, pending }`, so you get the rows AND the sentence that says they are missing, and',
      'a required `title` makes you name what failed. For a surface that owns its whole region,',
      '`<QueryState query={…} skeleton={…}>` does the same with a render prop.',
      '',
      'The rule is about the SHAPE, not the default. `const { data: x = [] } = useThing()`',
      'discards the query object on the line it was created, so `isError`, `error` and',
      '`refetch` are unreachable from that component FOR EVER — no later edit can add an error',
      'branch without first restructuring this line. Thirty-nine surfaces were in exactly that',
      'state; every one of them rendered a confident "there are none of these" over a 500.',
      '',
      'Keeping the query and defaulting from it (`const q = useThing()` … `q.data ?? []`) is',
      'fine and is NOT matched — the error is still reachable, and the surrounding code is',
      'expected to render it.',
    ],
  },
  {
    id: 'listquery-notice-dropped',
    // Scanned structurally — see scanDroppedNotices().
    scan: scanDroppedNotices,
    what: 'a `listQuery()` whose `notice` is taken and never rendered (or never taken)',
    fix: [
      'Render the notice. `{list.notice}` above the list when stale rows are still useful,',
      'or `list.failed ? list.notice : <the list>` when there is nothing to stand behind.',
      '',
      'THIS RULE IS THE ONE THE PREVIOUS ROUND NEEDED AND DID NOT HAVE. Every round so far',
      'moved an invariant into a shared function and then found the laundering waiting at the',
      'consumers: `closedToAgents` became one definition and `ticketState` re-asked the same',
      'question by hand; `queryFn` bodies stopped swallowing and thirty-nine call sites',
      're-flattened the rejection with `= []`. `listQuery()` is the same kind of move — it',
      'returns the rows AND the already-rendered failure — and the same laundering is one',
      'keystroke away: bind `rows`, skip `notice`, and the surface is back to reporting an',
      'outage as "you have none of these". A shared helper you are allowed to half-use is a',
      'convention, not an invariant. This makes it one.',
      '',
      'If a surface genuinely must not show this failure — a decorative count, a second read',
      'of something already reported beside it — bind the notice and render it somewhere quiet',
      "(`variant: 'inline'`) rather than dropping it. Silence is what the last eight rounds",
      'were about.',
    ],
  },
  {
    id: 'hand-rolled-popover-engine',
    // Scanned structurally — see scanPopoverEngines(). The match is a
    // CONJUNCTION of two signals, not a filename heuristic and not either
    // signal alone: a portal/fixed-position panel that carries its own
    // document-level outside listener. Either half by itself is legal all over
    // the tree — InfoTip portals a panel with no listener (hover opens it),
    // router.ts listens at the document with no panel — so matching one signal
    // would fire on neighbours, and matching a name would fire on nothing.
    scan: scanPopoverEngines,
    // The COMPLETE list of files allowed to hold both halves:
    allow: [
      // The three sanctioned shells — this rule's subject, not its exceptions.
      // They are the one owner of the four decisions every engine before them
      // made differently: outside click, Escape, what a scroll does, and how
      // the panel escapes a stacking context.
      'ui/src/components/ui/Popover.svelte',
      'ui/src/components/ui/DropdownMenu.svelte',
      'ui/src/components/ui/ContextMenu.svelte',
      // The ONE documented exception, kept hand-rolled on purpose (PR 7): an
      // EXTERNAL anchor (the editor's selection) with a measured flip, which
      // the primitive's trigger-anchored contract cannot express. If a second
      // file believes it needs this, the answer is a slot on Popover, not a
      // sixth entry here.
      'ui/src/components/ui/DocLinkPopover.svelte',
      // Not a popover at all — an INPUT primitive that trips the scan because
      // its suggestion list is a portaled panel with an outside listener. The
      // panel must keep keyboard focus in the search field, and its Escape
      // stops at the dropdown so it does not close the Modal hosting it; a
      // trigger/content split cannot express an input whose trigger IS the
      // panel's filter.
      'ui/src/components/ui/Combobox.svelte',
    ],
    what: 'a hand-rolled popover engine — a portal/fixed panel with its own document-level outside listener',
    fix: [
      'Use a shell: `<Popover>` for content panels, `<DropdownMenu>` for item lists,',
      '`<ContextMenu>` (via `useContextMenu()`) at the cursor. All three already answer the four',
      'questions a hand-rolled engine answers wrong: what an outside click closes, what Escape',
      'does, what a scroll does (close — the anchor moved, and guessing where to is worse; or',
      'follow, when the trigger is docked to fixed chrome), and how the panel stacks (fixed +',
      'portaled to <body> — cards carry backdrop-filter, and no z-index saves an absolutely',
      'positioned sibling).',
      '',
      'WHY THIS RULE: PR 7 deleted fourteen hand-rolled engines that disagreed on exactly those',
      'four questions — three different close behaviors among them, including a verbatim',
      'ContextMenu clone in InboxChatPanel. A new match here is engine fifteen. The allowlist',
      'above is complete (three shells, one documented exception, one input primitive); believing',
      'you need a sixth entry is a conversation for the PR, not a line in this file.',
    ],
  },
]

/** Legal in named files, in a known quantity, and forbidden everywhere else.
 *  A census entry is a promise: "these are all of them, and a new one is a bug."
 *  Counts are exact — a file that GAINS an occurrence fails, and a file that
 *  LOSES its last one fails too, so the table cannot rot into a permanent
 *  amnesty for code that was fixed years ago. */
/** The `hand-written-harness` census left with the port: every non-exempt call
 *  site it policed was in the deleted engines, and the persona-conversation
 *  path it carried as its one permanent exception (channel-replies.ts) serves
 *  from the Rust api now. Model access in the live TS tier goes through the
 *  transports (gateway.ts, llm-gateway.ts) and the one runner that may call
 *  them (harness/run.ts). */

const CENSUS = [
  {
    id: 'off-board-status-literal',
    // 'failed' next to 'cancelled' (either order) in an array literal or a type
    // union — i.e. a copy of OFF_BOARD_STATUSES.
    pattern: /(?:(['"])failed\1\s*[,|]\s*(['"])cancelled\2|(['"])cancelled\3\s*[,|]\s*(['"])failed\4)/g,
    what: "the off-board status list, spelled out as a literal",
    fix: [
      "Import the list: `import { OFF_BOARD_STATUSES } from '@/lib/task-const'`.",
      'This set decides which statuses exist but are never COLUMNS. A copy that drifts leaves',
      'tickets in a status no view draws — work that has silently disappeared off the board.',
      'The workflow-column engine that owns the set on the server side is the Rust statuses',
      'engine (api/src/statuses.rs); the client list is the wire vocabulary it serves.',
    ],
    sites: {
      // The one TS definition. `server/statuses.ts` carried the second until
      // the Rust cutover deleted the file — and with it the cross-check that
      // held the two lists identical. The Rust engine declares its own, and a
      // client copy must import, not re-spell.
      'ui/src/lib/task-const.ts': 1,
    },
  },
  {
    id: 'board-agent-policy-in-sql',
    // `allow_all_agents` inside a query — the board agent policy, expressed as
    // SQL instead of asked of the engine that owns it.
    pattern: /\ballow_all_agents\b/g,
    what: 'the board agent policy, re-expressed in SQL',
    fix: [
      'The policy is the Rust boards engine\'s (api/src/boards.rs — `board_allows_agent` and the',
      'set-scoping SQL fragment both live there). No TS query needs this column: the resident',
      'tier reads boards through the api, never straight out of the database.',
      '',
      'The one site below is the migration DDL that declares the column. Anything else in TS',
      'would be a THIRD implementation of a policy the api already owns end to end.',
    ],
    sites: {
      'ui/src/server/db/pg.ts': 1, // the column DDL
    },
  },
  {
    id: 'raw-client-fetch',
    // The ONE HTTP door for browser code: every request the app makes to its
    // own API goes through a verb in ui/src/lib/fetch-json.ts. Server files
    // fetch upstream services all the time (OAuth exchanges, MCP relays,
    // safeFetch) and the SDK is its own published door — both excluded below.
    pattern: /\bfetch\s*\(/g,
    what: 'a hand-rolled client fetch, bypassing the one HTTP door',
    fix: [
      'Use a verb from `@/lib/fetch-json`: `getJson` / `getList` / `getJsonOr404` / `getJsonOr` for',
      'reads, `postJson` / `putJson` / `patchJson` / `delJson` for mutations, `postStream` for SSE',
      'and other streaming replies, and the `*JsonOr` twins where a specific 4xx body is an ANSWER',
      'the surface renders (the endpoint cascade 409s, the focus actions, the Google-panel 409/502s).',
      '',
      'THIS RULE EXISTS BECAUSE THE STANZAS DRIFTED. The 2026-08 audit counted 134 hand-written',
      '`fetch(...)` blocks in client code, and they disagreed about the one thing that matters:',
      'most never read the error body (a failed POST resolved and the UI reported success), a few',
      'resolved the `{ error }` envelope AS the created record, and two streamed SSE with different',
      'frame rules than the chat parser. One door, one decision: non-2xx rejects with the server\'s',
      'own sentence, and every caller catches or deliberately swallows it in writing.',
      '',
      'The two counted files below are the doors themselves, at their exact call counts — if the',
      'door is renamed or emptied, the stale-census half of this check fails instead of the rule',
      'going quietly inert. A fetch anywhere else in client code is a new stanza. If you believe',
      'you need one, say so in the PR.',
    ],
    exempt: (path) =>
      path.startsWith('ui/src/server/') ||
      path.startsWith('ui/src/routes/api/') ||
      path.endsWith('.test.ts') ||
      !path.startsWith('ui/src/'), // the mcp/ and cli/ trees are not the browser app
    sites: {
      'ui/src/lib/fetch-json.ts': 5, // the door: getJson, getJsonOr404, getJsonOr, sendJson, postStream
      'ui/src/sdk/index.ts': 1, // the published SDK's own door — it cannot import the app's
    },
  },
  {
    id: 'same-origin-fetch-outside-the-door',
    // The credential stanza. `credentials: 'same-origin'` is what makes a fetch
    // a request AS THE SIGNED-IN USER — the difference between an anonymous
    // probe and a mutation on the viewer's session — so a second stanza is a
    // second door, and doors drift.
    pattern: /\bcredentials\s*:\s*['"]same-origin['"]/g,
    exempt: (path) =>
      !path.startsWith('ui/src/') || // server files fetch upstream; mcp/ and cli/ are not the browser app
      path.endsWith('.test.ts'), // a test may drive the door by hand; it is not a second one
    what: "`credentials: 'same-origin'` outside the one HTTP door",
    fix: [
      'Use a verb from `@/lib/fetch-json`: `getJson` / `getList` / `getJsonOr404` / `getJsonOr` for',
      'reads, `postJson` / `putJson` / `patchJson` / `delJson` (and the `*Or` twins) for mutations.',
      'Every one of them spreads the SAME_ORIGIN RequestInit defined once in that file — the',
      'stanza, the error-body contract and the JSON encoding are decided there, once, and a',
      'hand-rolled stanza inherits none of them when they change.',
      '',
      'THE DRIFT THIS CLOSES (audit 2026-08-26, P2): before the mutation door, 134 hand-rolled',
      "`credentials: 'same-origin'` stanzas across 67 files — and the stanza was the only part",
      'they AGREED on. The bodies around them did not: most never read the error body (a failed',
      'POST resolved and the UI reported success), and eight private helpers carried eight',
      'divergent error contracts. PR 2 collapsed all of it into fetch-json; the audit line was',
      '"Invariant rule: the stanza lives only in fetch-json.ts", and this census is that line,',
      'executable.',
      '',
      'The SDK entry below is not a second door in the app: it is the same exception',
      '`raw-client-fetch` above already carries — the published SDK cannot import the app\'s',
      'door, so it is its own, and its count is exact.',
    ],
    sites: {
      'ui/src/lib/fetch-json.ts': 1, // the door: the one SAME_ORIGIN RequestInit every verb spreads
      'ui/src/sdk/index.ts': 1, // the published SDK's own door — it cannot import the app's
    },
  },
]

// ── Machinery ────────────────────────────────────────────────────────────────

function walk(dir, out = []) {
  let entries
  try {
    entries = readdirSync(dir)
  } catch {
    return out
  }
  for (const name of entries) {
    const full = join(dir, name)
    if (statSync(full).isDirectory()) {
      if (!SKIP_DIRS.has(name)) walk(full, out)
    } else if (EXTS.some((e) => name.endsWith(e)) && !SKIP_FILES.has(name)) {
      out.push(full)
    }
  }
  return out
}

/** Blank out whole-line comments, preserving line count so reported line numbers
 *  stay honest. See KNOWN LIMIT at the top. */
function stripComments(src) {
  let inBlock = false
  return src
    .split('\n')
    .map((line) => {
      const t = line.trim()
      if (inBlock) {
        if (t.includes('*/')) inBlock = false
        return ''
      }
      if (t.startsWith('/*')) {
        if (!t.includes('*/')) inBlock = true
        return ''
      }
      if (t.startsWith('//') || t.startsWith('*')) return ''
      return line
    })
    .join('\n')
}

const lineOf = (src, index) => src.slice(0, index).split('\n').length

function matches(src, pattern) {
  const out = []
  const re = new RegExp(pattern.source, pattern.flags.includes('g') ? pattern.flags : pattern.flags + 'g')
  let m
  while ((m = re.exec(src)) !== null) {
    out.push({ line: lineOf(src, m.index), text: m[0].replace(/\s+/g, ' ').trim() })
    if (m[0].length === 0) re.lastIndex++
  }
  return out
}

/** Find each `queryFn` property and return its VALUE — the text from the `:` to
 *  the comma or brace that closes the property — so the swallow patterns below
 *  are judged inside a query function and nowhere else. `if (!r.ok) return null`
 *  in an imperative helper is a design choice; the same line inside a queryFn is
 *  a failed request reported as a successful empty one. */
function queryFnBodies(src) {
  const bodies = []
  const re = /\bqueryFn\b\s*:/g
  let m
  while ((m = re.exec(src)) !== null) {
    let i = re.lastIndex
    let depth = 0
    const start = i
    for (; i < src.length; i++) {
      const c = src[i]
      if (c === '(' || c === '[' || c === '{') depth++
      else if (c === ')' || c === ']' || c === '}') {
        if (depth === 0) break // the brace closing the useQuery options object
        depth--
      } else if (c === ',' && depth === 0) break
    }
    bodies.push({ text: src.slice(start, i), offset: start })
  }
  return bodies
}

// An "empty" is not only `[]` / `null` / `{}`. The #202 merge shipped
// `if (!response.ok) return { agents: [] }` — a WRAPPED empty, which the first
// two patterns below missed entirely, so a failed /api/mcp read told the assistant
// composer the agent had no MCP servers. Any object literal whose values are
// all empty arrays/nulls is the same lie, so match that shape too.
const EMPTY = String.raw`(?:\[\s*\]|null|undefined|\{\s*\}|\{[^{}]*:\s*(?:\[\s*\]|null)\s*(?:,[^{}]*:\s*(?:\[\s*\]|null)\s*)*\})`
const SWALLOWS = [
  { re: new RegExp(String.raw`if\s*\(\s*!\s*[\w.]+\.ok\s*\)\s*return\s*${EMPTY}`, 'g'), note: 'if (!r.ok) return <empty>' },
  { re: new RegExp(String.raw`\.catch\s*\(\s*\(\s*\)\s*=>\s*\(?\s*${EMPTY}`, 'g'), note: '.catch(() => <empty>)' },
  // `r.ok ? r.json() : <empty>` — the ternary spelling of the same thing.
  { re: new RegExp(String.raw`\.ok\s*\?[^:]{0,80}:\s*\(?\s*${EMPTY}`, 'g'), note: 'r.ok ? … : <empty>' },
]

function scanQueryFns(src) {
  const hits = []
  for (const body of queryFnBodies(src)) {
    for (const { re, note } of SWALLOWS) {
      for (const h of matches(body.text, re)) {
        hits.push({ line: lineOf(src, body.offset) + h.line - 1, text: `${note} — ${h.text}` })
      }
    }
    // The ternary form: `r.ok ? r.json() : []`. Split from the regexes above
    // because a ternary's own `:` defeats a single-pass pattern.
    if (/\.ok\s*\?/.test(body.text) && /:\s*\(?\s*(?:\[\s*\]|null|\{\s*\}|undefined)\s*\)?\s*[),]/.test(body.text)) {
      hits.push({ line: lineOf(src, body.offset), text: 'r.ok ? … : <empty> inside a queryFn' })
    }
  }
  return hits
}

/** Find the places where a `use*()` result is consumed and thrown away in the
 *  same expression, so nothing downstream can ever see that the read failed.
 *
 *  Two spellings, one bug:
 *    `const { data: xs = [] } = useUsers()`   — destructured with a default
 *    `const xs = useUsers().data ?? []`        — read straight off the call
 *
 *  Both produce `[]` for a 200-with-no-rows AND for a 500, and both leave
 *  `isError`/`error`/`refetch` with no name to reach them by. `null` and `{}`
 *  defaults are matched too: a missing object renders as "not set" just as
 *  loudly as a missing list renders as "none".
 *
 *  NOT matched: `const q = useUsers()` followed by `q.data ?? []`. That keeps
 *  the query, so the error branch is one line away rather than impossible. */
function scanDiscardedQueries(src) {
  const hits = []
  // `{ … = [] … } = useThing(`. Bounded runs of non-brace text keep this from
  // wandering across a whole file when a line happens not to match.
  const destructured = /\{[^{}]{0,400}?=\s*(?:\[\s*\]|null|undefined)[^{}]{0,400}?\}\s*=\s*(use[A-Z]\w*)\s*\(/g
  for (const h of matches(src, destructured)) hits.push({ ...h, text: `defaulted off ${h.text.slice(-40)}` })
  // `useThing().data ?? []` / `useThing(x).data?.y ?? []`.
  const inlined = /\buse[A-Z]\w*\s*\([^()]{0,200}\)\s*\.\s*data\b[^;\n]{0,80}\?\?\s*(?:\[\s*\]|\{\s*\}|null)/g
  for (const h of matches(src, inlined)) hits.push(h)
  return hits
}

/** Top-level declaration starts, used as a cheap scope boundary. Every consumer
 *  of `listQuery` is a React component declared at the top level of its module,
 *  so "from this binding to the next top-level declaration" is the component
 *  body closely enough to tell a rendered notice from a dropped one — and it
 *  keeps five components in one file (Home's tabs) from covering for each other. */
const TOP_LEVEL_DECL = /^(?:export\s+)?(?:default\s+)?(?:async\s+)?(?:function|const|let|var|class)\s/gm

function scopeAfter(src, index) {
  TOP_LEVEL_DECL.lastIndex = index
  const m = TOP_LEVEL_DECL.exec(src)
  return src.slice(index, m ? m.index : src.length)
}

/** `listQuery()` returns the rows AND the failure. Nothing in JavaScript makes a
 *  caller RENDER the failure, so this does.
 *
 *  Three ways to drop it, all reported:
 *    `const { rows } = listQuery(…)`            — never bound
 *    `const { rows, notice } = listQuery(…)`    — bound, never used again
 *    `const l = listQuery(…)` with no `l.notice` anywhere in the file
 *
 *  A dropped notice is not a smaller bug than the one `listQuery` was built to
 *  fix — it IS that bug, with an extra line of ceremony in front of it. */
function scanDroppedNotices(src) {
  const hits = []
  const re = /\bconst\s+(\{[^{}]*\}|[A-Za-z_$][\w$]*)\s*=\s*listQuery\s*\(/g
  let m
  while ((m = re.exec(src)) !== null) {
    const target = m[1]
    const line = lineOf(src, m.index)
    if (!target.startsWith('{')) {
      // Named result: `list.notice` may legitimately be rendered anywhere in the
      // file (a helper below the component, a prop passed down), so the whole
      // file is the scope. The name is unique, so this stays precise.
      const used = new RegExp(`\\b${target.replace(/\$/g, '\\$')}\\s*\\.\\s*notice\\b`).test(src)
      if (!used) hits.push({ line, text: `${target} = listQuery(…) — \`${target}.notice\` is never rendered` })
      continue
    }
    // Destructured: find what `notice` was bound AS (it may be renamed).
    const bind = /(^|[{,\s])notice\s*(?::\s*([A-Za-z_$][\w$]*))?\s*(?=[,}])/.exec(target)
    if (!bind) {
      hits.push({ line, text: `${target.replace(/\s+/g, ' ')} = listQuery(…) — \`notice\` is not even bound` })
      continue
    }
    const local = bind[2] ?? 'notice'
    // Look forward from the END of the destructuring pattern to the next
    // top-level declaration: a plain `notice` binding must be *used* after it.
    const after = scopeAfter(src, m.index + m[0].length)
    if (!new RegExp(`\\b${local}\\b`).test(after))
      hits.push({ line, text: `\`${local}\` is bound from listQuery(…) and never rendered` })
  }
  return hits
}

/** The two halves of a hand-rolled popover engine, as a conjunction:
 *
 *    PANEL    a portaled or fixed-position panel — `use:portal`, an import of
 *             the portal action, or `position: fixed` in a style string
 *    OUTSIDE  the file's own document-level pointer listener, in either
 *             spelling: `document.addEventListener('mousedown', …)` or
 *             `<svelte:document onmousedown={…}>` — window/body too, and
 *             pointerdown/click as well as mousedown, because the event is
 *             not the point; owning the outside-close IS.
 *
 *  Either half alone is common and legal — a hover tooltip portals a panel
 *  with no listener, router.ts listens at the document with no panel — so an
 *  engine is a file that holds BOTH, and one hit is reported per file.
 *
 *  NOT matched, knowingly: a panel made `fixed` only by a Tailwind class, or
 *  portaled by a raw appendChild, in a file with no `position: fixed` string,
 *  no `use:portal` and no portal import. No such engine exists in the tree and
 *  all five sanctioned files use the shared action; matching every mention of
 *  `fixed` would fire on half the CSS in the app. Same trade as the dynamic-
 *  import spellings above: catch what people actually write, and let the
 *  allowlist be the argument about the rest. */
function scanPopoverEngines(src) {
  const PANEL = /\buse:portal\b|position:\s*fixed|from\s+['"]@\/lib\/portal['"]/
  const OUTSIDE =
    /(?:document|window)\s*\.\s*addEventListener\s*\(\s*['"](?:mousedown|pointerdown|click)['"]|<svelte:(?:document|window|body)\b[^>]*\bon:?(?:mousedown|pointerdown|click)=/
  if (!PANEL.test(src)) return []
  const hits = matches(src, OUTSIDE)
  if (!hits.length) return []
  return [{ line: hits[0].line, text: `panel + own document-level listener (${hits[0].text})` }]
}

// ── Run ──────────────────────────────────────────────────────────────────────

const failures = []
const notes = []

const files = SOURCE_DIRS.flatMap((d) => walk(join(ROOT, d)))
const sources = new Map() // repo-relative path -> comment-stripped text
for (const f of files) sources.set(relative(ROOT, f).split(sep).join('/'), stripComments(readFileSync(f, 'utf8')))

// Rules: forbidden outside `allow`.
for (const rule of RULES) {
  const found = []
  for (const [path, src] of sources) {
    if (rule.allow?.includes(path)) continue
    const hits = rule.scan ? rule.scan(src) : matches(src, rule.pattern)
    for (const h of hits) found.push({ path, ...h })
  }
  if (found.length) failures.push({ id: rule.id, what: rule.what, fix: rule.fix, found })
}

// The popover rule's `allow` list carries the same rot risk every `allow` does,
// so it gets the same companion: each of its five files must still EXIST and
// still hold both halves of an engine. When `closedToAgents` was renamed, its
// duplicate rule kept passing on a tree that no longer contained it; a renamed
// shell would leave an entry that exempts nothing while reading as a sanctioned
// engine — which is how an allowlist becomes a standing amnesty.
{
  const popoverRule = RULES.find((r) => r.id === 'hand-rolled-popover-engine')
  for (const path of popoverRule.allow) {
    const src = sources.get(path)
    if (src === undefined) {
      failures.push({
        id: 'popover-allowlist-entry-gone',
        what: `${path} is on the popover rule's allowlist but no longer exists`,
        fix: [
          "If the shell moved or was renamed, update `allow` on 'hand-rolled-popover-engine' in",
          'scripts/check-invariants.mjs. The allowlist names real files, and an entry that points',
          'at nothing exempts nothing while still reading as a sanctioned engine.',
        ],
        found: [],
      })
    } else if (!scanPopoverEngines(src).length) {
      failures.push({
        id: 'popover-allowlist-entry-is-not-an-engine',
        what: `${path} is on the popover rule's allowlist but no longer carries an engine`,
        fix: [
          'It holds neither a portal/fixed panel nor a document-level outside listener any more —',
          'most likely it was rebuilt on one of the shells. Delete its entry from `allow`; an',
          'exemption held open for a file that no longer needs it is held open for the next',
          'hand-rolled engine to walk through.',
        ],
        found: [],
      })
    }
  }
}

// A BACKTICK INSIDE A SQL COMMENT IN pg.ts, which has now broken the tree twice
// in one day — the same shape, the same file, found both times by a bystander.
//
// `MIGRATIONS` is an array of BACKTICK template literals holding SQL. A `--`
// comment inside one of those strings is still inside the string, so quoting an
// identifier the way the surrounding TypeScript does terminates the literal
// early. What you get is not an error at the offending line: the rest of the
// array becomes a string, the file stops parsing, and ~51 test files fail to
// import with errors pointing nowhere near the edit. That is a genuinely
// expensive twenty minutes for anyone who did not write the comment.
//
// It reads the file RAW rather than from `sources`, because that map is
// comment-stripped and this bug lives entirely in a comment.
//
// This is not a style rule. Single quotes read identically in a SQL comment and
// cannot end the string.
{
  const PG = 'ui/src/server/db/pg.ts'
  const raw = readFileSync(join(ROOT, PG), 'utf8')
  const start = raw.indexOf('const MIGRATIONS: string[] = [')
  if (start === -1) {
    failures.push({
      id: 'pg-migrations-array-missing',
      what: `the MIGRATIONS array was not found in ${PG}; the backtick check below is now inert`,
      fix: ['If the array moved or was renamed, update this check in scripts/check-invariants.mjs.'],
      found: [],
    })
  } else {
    const body = raw.slice(start, raw.indexOf('\n]', start))
    const offenders = []
    let inside = false
    body.split('\n').forEach((line, i) => {
      const trimmed = line.trim()
      if (inside && trimmed.startsWith('--') && line.includes('`')) {
        offenders.push({ line: i + 1, text: trimmed.slice(0, 100) })
      }
      // Odd number of backticks on a line flips whether we are inside a literal.
      if ((line.match(/`/g) ?? []).length % 2 === 1) inside = !inside
    })
    if (inside) {
      failures.push({
        id: 'pg-migrations-unterminated-literal',
        what: `${PG}'s MIGRATIONS array has an unterminated template literal — the file will not parse`,
        fix: [
          'A backtick inside a sql `--` comment ended the string early. Find it and use single',
          'quotes instead. Nothing else in the repo will typecheck until this is fixed, and the',
          'errors it reports will point at unrelated files.',
        ],
        found: [],
      })
    }
    if (offenders.length) {
      failures.push({
        id: 'backtick-in-pg-sql-comment',
        what: 'a backtick quoting an identifier inside a sql comment in the MIGRATIONS array',
        fix: [
          'Use single quotes. The array is delimited by backticks, so inside it a backtick ends',
          "the string — the file stops parsing and ~51 test files fail to import with errors",
          'nowhere near the edit. This has broken the tree twice; the comment reads the same',
          'either way.',
        ],
        found: offenders.map((o) => ({ path: PG, line: o.line, text: o.text })),
      })
    }
  }
}

// Destructive-statement guard on the MIGRATIONS array. Every statement is one
// transaction away from every customer database, and most of the people who
// will ever append one are writing SQL under time pressure at the end of a
// feature. The patterns below are the ones that destroy or rewrite data that
// no checksum can bring back: unmarked, CI fails and the second look happens
// before the merge, not after a customer boot.
//
// The escape hatch is the marker. A statement carrying an inline
// `-- deliberate: <why>` comment passes every pattern — the marker IS the
// second look, recorded in the array next to the statement it justifies.
//
// History is exempt (GUARD_FROM_INDEX below): the first 304 statements
// predate the guard, and annotating them now would churn the array's checksums
// for zero new safety — the checksum law means an edited statement refuses to
// boot, so the past is already frozen. The number is a literal on purpose and
// never auto-updates: every future destructive statement demands its marker,
// forever.
{
  const PG = 'ui/src/server/db/pg.ts'
  const GUARD_FROM_INDEX = 304
  const MARKER = /--\s*deliberate:\s*\S/
  // Ordered [test, name] pairs. Drop-index/view/constraint are deliberately
  // absent: those are routine, reversible cleanup, and the CI schema snapshot
  // (ui/src/server/db/schema.snapshot.sql) already puts them in the PR diff —
  // a tripwire that fires on cleanup noise gets widened until it is gone.
  const PATTERNS = [
    [/\bdrop\s+(table|column|database|schema|type)\b/, 'drop table/column/database/schema/type'],
    [/\btruncate\b/, 'truncate'],
    [/\bdelete\s+from\b/, 'delete from (no where)'],
    [/\bupdate\b[^;]*\bset\b/, 'update … set (no where)'],
    [/\balter\s+column\s+[^;]*\b(type|using)\b/, 'alter column type/using'],
    [/\brename\s+(to|column)\b/, 'rename (api queries reference the name)'],
  ]
  // Whole-line TS comments are blanked before backtick pairing — the prose
  // between entries quotes identifiers in backticks (`schema_migrations`) and
  // would otherwise pair with the next statement's opening delimiter and
  // inflate the count. SQL `--` comments live INSIDE statements and survive
  // (stripComments only blanks whole-line // and /* */ comments).
  const stripped = stripComments(readFileSync(join(ROOT, PG), 'utf8'))
  const start = stripped.indexOf('const MIGRATIONS: string[] = [')
  const body = stripped.slice(start, stripped.indexOf('\n]', start))
  // Safe to split on backticks: the checks above guarantee no stray backtick
  // lives anywhere in the array.
  const statements = [...body.matchAll(/`([^`]*)`/g)]
  if (statements.length >= GUARD_FROM_INDEX) {
    const offenders = []
    statements.forEach(([text, sql], index) => {
      if (index < GUARD_FROM_INDEX || MARKER.test(text)) return
      // Normalize: blank single-quoted literals first (so a data value that
      // says 'truncate this' cannot fire the guard), THEN strip -- comments,
      // then lowercase for the case-insensitive patterns.
      const norm = sql
        .replace(/'(?:[^'\\]|'')*'/g, "''")
        .replace(/--[^\n]*/g, '')
        .toLowerCase()
      for (const [test, name] of PATTERNS) {
        // delete/update only count when the statement has no where clause —
        // a scoped delete is a repair, an unscoped one is a wipe.
        if ((name.includes('no where') && /\bwhere\b/.test(norm)) || !test.test(norm)) continue
        offenders.push({
          line: lineOf(stripped, statements[index].index ?? 0),
          text: `${name}: ${text.trim().split('\n')[0].slice(0, 80)}`,
        })
        break
      }
    })
    if (offenders.length) {
      failures.push({
        id: 'pg-migration-destructive-unmarked',
        what: `a MIGRATIONS statement (index >= ${GUARD_FROM_INDEX}) can destroy or rewrite data and carries no \`-- deliberate:\` marker`,
        fix: [
          'Add a `-- deliberate: <why>` comment INSIDE the statement so the decision travels with',
          'the SQL — or make the change non-destructively (add-first, drop in a later release).',
          `The guard starts at statement ${GUARD_FROM_INDEX}; history before it is frozen by the checksum law.`,
        ],
        found: offenders.map((o) => ({ path: PG, line: o.line, text: o.text })),
      })
    }
  } else if (statements.length > 0) {
    // The array shrank below the guard's baseline. Statements cannot be removed
    // (the checksum refuses to boot on a database that applied them), so this
    // is tampering or a parsing regression — either way the guard is inert and
    // must not pass silently.
    failures.push({
      id: 'pg-migration-guard-baseline-lost',
      what: `the MIGRATIONS array parses as ${statements.length} statements, below the destructive guard's baseline of ${GUARD_FROM_INDEX}`,
      fix: ['Statements are append-only — find what removed entries, or fix the parser in scripts/check-invariants.mjs.'],
      found: [],
    })
  }
}

// Census: exact counts per named file, forbidden anywhere else.
for (const rule of CENSUS) {
  const found = []
  const stale = []
  const counted = new Map()
  for (const [path, src] of sources) {
    // `exempt` is for files where the pattern is the SUBJECT rather than a copy
    // of it: the module that defines the thing, and the tests that drive it.
    // A census entry says "this is a copy and it is owed"; an exemption says
    // "this is not a copy at all", and conflating the two would put the
    // definition on a debt list that can never reach zero.
    if (rule.exempt?.(path)) continue
    const hits = matches(src, rule.pattern)
    if (hits.length) counted.set(path, hits)
    const allowed = rule.sites[path] ?? 0
    if (hits.length > allowed) {
      for (const h of hits.slice(allowed)) found.push({ path, ...h })
    }
  }
  for (const [path, allowed] of Object.entries(rule.sites)) {
    const n = counted.get(path)?.length ?? 0
    if (n < allowed) stale.push(`${path}: census says ${allowed}, found ${n}`)
  }
  if (found.length) failures.push({ id: rule.id, what: rule.what, fix: rule.fix, found })
  if (stale.length) {
    failures.push({
      id: `${rule.id}-census-stale`,
      what: 'the census over-counts — a listed occurrence is gone',
      fix: [
        'Lower or remove the count in scripts/check-invariants.mjs. The census is exact so it',
        'shrinks as the debt is paid and cannot become a standing amnesty.',
        ...stale.map((s) => `  ${s}`),
      ],
      found: [],
    })
  }
}

// EVERY TOOL AN AGENT CAN CALL MUST BE SIMULATED AND EXERCISED — and the
// invariant lives in the Rust tree now, with the toolbox it describes. Four
// #[test]s in api/src/fitness/toolbox/ carry it, unit-level so a new
// registration fails in `cargo test` first:
//
//   MODELLED    every `registerTool` in mcp/src/index.ts appears in the fitness
//               catalog with the real description and argument names
//               (talaria_tools.rs — `models_every_tool_the_toolkit_registers`,
//               under its own reads-the-real-source guard)
//   BACKED      every catalogued tool has a simulated backend in the sandbox,
//               and every backend is in the catalog (sandbox.rs —
//               `every_catalog_tool_is_backed_and_every_backend_is_in_the_catalog`)
//   EXERCISED   every backend is driven by a sandbox test or a harness's
//               dry-run tool surface (sandbox.rs —
//               `every_backend_is_exercised_by_a_test_or_a_harness_surface`)
//
// WHY AN ANCHOR CHECK INSTEAD OF THE CENSUS ITSELF. This script used to compute
// the census over the TS toolbox; the toolbox moved to Rust with the port and
// the census went with it. What belongs HERE is the lesson every renamed rule
// in this file has paid for once: a coverage check whose subject quietly moved
// reports "all clean" over nothing. The anchors fail when a test is renamed,
// deleted, or moved — and the fix text points at the file that holds the rule.
{
  const ANCHORS = [
    ['api/src/fitness/toolbox/talaria_tools.rs', 'fn reads_the_real_registrations_at_all'],
    ['api/src/fitness/toolbox/talaria_tools.rs', 'fn models_every_tool_the_toolkit_registers'],
    ['api/src/fitness/toolbox/sandbox.rs', 'fn every_catalog_tool_is_backed_and_every_backend_is_in_the_catalog'],
    ['api/src/fitness/toolbox/sandbox.rs', 'fn every_backend_is_exercised_by_a_test_or_a_harness_surface'],
  ]
  const missing = ANCHORS.filter(([path, anchor]) => !readFileSync(join(ROOT, path), 'utf8').includes(anchor))
  if (missing.length) {
    failures.push({
      id: 'toolkit-coverage-anchor-missing',
      what: 'the fitness toolbox coverage tests are not where this check expects them',
      fix: [
        'The toolkit coverage invariant (every registered tool modelled, simulated, and exercised)',
        'lives in #[test]s in api/src/fitness/toolbox/. A named anchor was not found:',
        ...missing.map(([path, anchor]) => `  ${anchor}  in  ${path}`),
        '',
        'If the tests moved or were renamed, update ANCHORS in scripts/check-invariants.mjs — do',
        'NOT delete this check. It is what fails when the coverage rule quietly stops existing;',
        'the Rust suite is what fails when a tool is uncovered.',
      ],
      found: [],
    })
  }
}

// A BROWSER MODULE MAY NAME A SERVER TYPE. IT MAY NOT IMPORT SERVER CODE.
//
// `import type { EvalCaseScore } from '@/server/fitness/evals'` is erased at
// build time and is how the payload contract stays a contract. Drop the `type`
// and the same line pulls the sweep driver — and transitively the database
// pool, the harness runner and the guard registry — into the browser bundle.
//
// THIS SHIPPED. One value import of a single number constant took the whole
// Models route down; `components/models/fitness.ts` even carries a header
// saying it is runtime-dependency-free on purpose, and the header did not stop
// it. The constant is a literal now with a test holding the two in step, which
// is the right shape for the handful of cases that genuinely need a value.
//
// Tests are exempt: they run in node and importing the real module is how a
// copy is held to its original.
{
  // `(?!\bimport\b)` and not a bare `[^;]*?`: this codebase does not use
  // semicolons, so the original could start at one import statement and run all
  // the way to a LATER one's `from '@/server/…'`. The first browser module to
  // carry a legitimate `import type … from '@/server/…'` behind any other
  // import was reported as a value import — a false positive that invites
  // exactly the two responses this file's own footer warns against (contorting
  // correct code, or widening the pattern until it passes).
  //
  // This narrows rather than widens: a real value import is still matched,
  // because the offending `import` and its `from` are the same statement with
  // no `import` keyword between them.
  const IMPORT_FROM_SERVER = /\bimport\s+(?!type\b)(?:(?!\bimport\b)[^;])*?\sfrom\s*['"]@\/server\/[^'"]+['"]/g
  // `ui/src/lib/` joined this list once the pattern above could tell a type
  // import from a value one. It is browser code by default — `cn`, the motion
  // and theme signals, the query wrappers — and it was the one obvious hole:
  // a value import there ships the database pool into the client bundle just
  // as surely as one in a component, and nothing was watching. Under the OLD
  // pattern adding it would have failed two correct files immediately
  // (`inbox-focus.svelte.ts`, `session.ts`, both type-only server imports
  // sitting behind another import), which is why the narrowing had to land
  // first. `ui/src/routes/api/` is deliberately NOT here: those ARE the server.
  const BROWSER = ['ui/src/components/', 'ui/src/routes/app/', 'ui/src/lib/']
  // THE LAST FALSE POSITIVE, and it is older than either edit to the pattern
  // above: `import { type A } from '@/server/x'` — inline `type` on every
  // binding, no `import type` prefix — is erased at compile time exactly like
  // the prefixed spelling, and matches anyway. Nothing in the tree writes it
  // that way today, which is precisely why it is worth closing now: it bites
  // the next person to write one, and what it hands them is the contort-or-
  // widen dilemma this whole block exists to avoid.
  //
  // A regex cannot express "every binding is type-prefixed" (it is a property
  // of a list), so the decision is made on the matched text instead. Only the
  // braced form can be type-only; a default or namespace import is a value by
  // construction, and an unparseable clause is treated as a violation — the
  // safe direction to be wrong in, since the cost is a question rather than a
  // database pool in the browser bundle.
  // THE OTHER DIRECTION, and by the asymmetry above it is the one that matters
  // more: four shapes reached `@/server/` without the word `from` in an import
  // clause, so none of them matched, and every one of them pulls the SAME
  // module graph. A side-effect import evaluates it; a dynamic import puts it
  // in a lazy chunk that fails when called rather than at load; a re-export
  // hands it to whoever imports the browser module next, which is worse than
  // keeping it, because the pool arrives somewhere that never named `@/server/`
  // at all. `type` guards on the re-export because `export type { … } from` is
  // erased exactly like the import spelling.
  const SIDE_EFFECT_FROM_SERVER = /\bimport\s*['"]@\/server\/[^'"]+['"]/g
  // The dynamic form is matched only in its two RUNTIME spellings, `await
  // import(…)` and `import(…).then(`, and that is a deliberate under-reach.
  // `import('@/server/x').SomeType` is a TYPE-position import expression — it
  // is erased, `lib/inbox-focus.svelte.ts:96` already writes one, and a regex
  // cannot tell it from a call by its prefix. Matching every `import(` would
  // have failed that correct line, which is the one outcome this block is
  // built to avoid: a rule that fires on correct code leaves both responses
  // (contort it, or widen the rule) worse than having no rule.
  // The runtime spellings of a dynamic import, and ONLY those.
  //
  // Matching every `import(` fires on correct code: `lib/inbox-focus.svelte.ts`
  // writes `import('@/server/inbox-focus').InboxTimelineEntry` in TYPE
  // position, which is erased, and no regex can tell that from a call by its
  // prefix alone — the same shape of limit as "every binding is type-prefixed"
  // being a property of a list rather than of a string.
  //
  // So each alternative below is a context that is RUNTIME BY GRAMMAR, where a
  // type can never appear:
  //   await import(…)          an expression, awaited
  //   import(…).then(          an expression, chained
  //   return import(…)         `return` takes a value
  //   void import(…)           `void` takes a value
  //   const|let|var x = import(…)   a value binding; a type alias says `type`
  //
  // That leaves a KNOWING under-reach — `() => import('@/server/x')` is not
  // here, because `=>` appears in type position too (`type F = () => import(…).T`)
  // and including it would fire on correct code again. An exotic runtime
  // spelling can still slip through. That is the deliberate trade: this rule
  // catches what people actually write, and the alternative — matching every
  // `import(` and declaring the one legitimate line a census exception — puts
  // friction on correct code to buy coverage of a form nobody writes.
  const DYNAMIC_FROM_SERVER = new RegExp(
    [
      /\bawait\s+import\s*\(\s*['"]@\/server\/[^'"]+['"]/,
      /\bimport\s*\(\s*['"]@\/server\/[^'"]+['"]\s*\)\s*\.\s*then\b/,
      /\breturn\s+import\s*\(\s*['"]@\/server\/[^'"]+['"]/,
      /\bvoid\s+import\s*\(\s*['"]@\/server\/[^'"]+['"]/,
      /\b(?:const|let|var)\s+[\w{}[\],\s]+=\s*import\s*\(\s*['"]@\/server\/[^'"]+['"]/,
    ]
      .map((r) => r.source)
      .join('|'),
    'g',
  )
  const REEXPORT_FROM_SERVER = /\bexport\s+(?!type\b)(?:(?!\bexport\b)[^;])*?\sfrom\s*['"]@\/server\/[^'"]+['"]/g

  // Braced form only, and now for `export { … } from` too — a re-export can be
  // inline-type-only in exactly the same way an import can.
  const typeOnlyClause = (text) => {
    const brace = /^(?:import|export)\s*\{([^}]*)\}\s*from\b/.exec(text)
    if (!brace) return false
    const bindings = brace[1]
      .split(',')
      .map((b) => b.trim())
      .filter(Boolean)
    return bindings.length > 0 && bindings.every((b) => /^type\s+\S/.test(b))
  }

  const found = []
  for (const [path, src] of sources) {
    if (!BROWSER.some((dir) => path.startsWith(dir))) continue
    if (path.endsWith('.test.ts')) continue
    for (const re of [IMPORT_FROM_SERVER, REEXPORT_FROM_SERVER, SIDE_EFFECT_FROM_SERVER, DYNAMIC_FROM_SERVER]) {
      for (const hit of matches(src, re)) {
        if (typeOnlyClause(hit.text)) continue
        found.push({ path, ...hit })
      }
    }
  }
  if (found.length) {
    failures.push({
      id: 'server-value-import-in-a-browser-module',
      what: 'a browser module imports server CODE, not just server types',
      fix: [
        "Make it `import type { … } from '@/server/…'`. A type import is erased; a value import is a",
        'module graph, and the graph under `@/server/` reaches the database pool, the harness runner',
        'and the guard registry. The route stops loading.',
        '',
        'IF YOU GENUINELY NEED A VALUE — a shared constant, an enum — declare it in the browser module',
        'and add a test that imports the server module and asserts the two agree. A test runs in node',
        'and may import anything; the browser may not. `components/models/fitness.ts` does exactly',
        'this for DEFAULT_CONCURRENCY.',
      ],
      found,
    })
  }
}

// UPLOAD BYTES HAVE ONE SERVING PATH, AND THE INLINE DECISION IS NOT A ROUTE'S.
//
// THE RULE. No route under ui/src/routes/api/ may (a) set a Content-Disposition
// that says `inline`, or (b) set a Content-Type taken from a stored `.mime` /
// `.type` field. Upload bytes go through `serve_upload` in api/src/uploads.rs —
// the single inline/download decision, built on the INLINE_MIME set (raster
// images and PDF inline; EVERYTHING else `attachment` + `nosniff` + a sandbox
// CSP). A route that re-makes that decision is the P0 again.
//
// THE FINDING THIS KEEPS FROM RETURNING (audit 2026-08-26, P0): both bytes
// routes served the uploader-declared MIME with `inline` disposition for any
// `image/*` and `text/*`. An upload's MIME is whatever the uploader's client
// SAID it was — the upload route stores `file.type` verbatim — so `text/html`
// and `image/svg+xml` rode the allowlist, and an inline response is
// SAME-ORIGIN: script in the "image" ran with the viewer's session. The public
// artifact route served it unauthenticated. The fix was one function and two
// one-line route changes; the bug was two routes each re-making a security
// decision that had already been made correctly in one place. A copy of that
// decision in a third file is what this block fails on.
//
// THE BOUNDARY, for the day a route legitimately sets a mime-typed content-type
// again: agent-media.$model.ts used to — `'content-type': media.mime` — and its
// mime was NOT uploader input. `readAgentImage` derived it from a fixed
// extension map (png/jpg/gif/webp — nothing a browser executes), path-guarded
// the read to the agent's own volume, and carried nosniff: a server-decided
// type on guarded bytes is the shape serve_upload itself has, so it was excepted
// BY NAME. The route moved to Rust with the cutover and the carve-out left with
// it — a bytes route that needs the same standing makes its case in the PR, in
// the fix text below.
{
  // The two bytes routes this block was written after are the Rust api's now
  // (R10): their TS files are deleted, and the one serving function they must
  // go through is `serve_upload` in api/src/uploads.rs — the INLINE_MIME
  // allowlist (raster + PDF inline, everything else attachment + nosniff +
  // sandbox CSP), unit-tested in that file. The scan below keeps watching the
  // resident routes that remain under ui/src/routes/api/; the companion follows
  // the bytes across the language line, because a guard that stops at the edge
  // of the tree its subject moved out of guards nothing.
  const RUST_BYTES_ROUTES = [
    'api/src/routes/files/uploads_id.rs',
    'api/src/routes/files/artifacts_public_slug_download.rs',
  ]
  const RUST_SERVE = 'api/src/uploads.rs'

  // (a) The disposition, any spelling that names the header and says inline —
  // including a ternary `` `${x ? 'inline' : 'attachment'}` ``: the DECISION is
  // the bug, whether the inline arm is a constant or a branch. `attachment`
  // alone does not match; that is the safe direction to miss in.
  const INLINE_DISPOSITION = /['"]content-disposition['"]\s*[:,=][^;\n]{0,120}\binline\b/i
  // (b) A content-type taken from a stored mime/type field: `up.mime`, `r.mime`,
  // `file.type`. A literal (`'application/json'`) does not match; an echoed
  // request/upstream header (`request.headers.get('content-type')`) does not —
  // parens are not part of an identifier chain, and a proxied type is not a
  // stored one.
  const STORED_MIME_TYPE = /['"]content-type['"]\s*[:,]\s*[A-Za-z_$][\w$.]*\b(?:mime|type)\b/i

  const found = []
  for (const [path, src] of sources) {
    if (!path.startsWith('ui/src/routes/api/') || path.endsWith('.test.ts')) continue
    for (const re of [INLINE_DISPOSITION, STORED_MIME_TYPE]) {
      for (const hit of matches(src, re)) found.push({ path, ...hit })
    }
  }
  if (found.length) {
    failures.push({
      id: 'upload-bytes-served-outside-serveupload',
      what: 'an inline or content-type decision on upload bytes, made inside a route',
      fix: [
        'Upload bytes are the Rust api\'s to serve: `serve_upload` in api/src/uploads.rs is the one',
        'inline/download decision — the INLINE_MIME allowlist (raster + PDF), the filename-header',
        'scrub, `nosniff`, and the sandbox CSP on everything downloaded. No route can widen it,',
        'which is the point: the two routes this rule was written after each shipped script-in-"image"',
        "running with the viewer's session, and the public one served it to the open internet.",
        '',
        'A TS resident route has no business making this decision at all — proxy the bytes to the',
        'api and let serve_upload answer. If the bytes are NOT an upload (generated on the server,',
        'read out of an agent volume), do what agent-media did: derive the type from a fixed map of',
        'safe extensions, refuse everything else, carry nosniff — and say so in the PR, because the',
        'exception has to be written into this block by name, not slipped past it.',
      ],
      found,
    })
  }

  // THE COMPANION — the closedToAgents lesson, applied to a helper. The scan
  // above watches every resident route file for a NEW hand-made decision; this
  // watches the two Rust routes that actually carry bytes for the helper
  // itself. If either stops calling serve_upload, the bytes came back inside
  // the route, and the scan above is the only thing left between the response
  // headers and the uploader's declared MIME.
  const rustSrc = (path) => {
    try {
      return readFileSync(join(ROOT, path), 'utf8')
    } catch {
      return undefined
    }
  }
  for (const path of RUST_BYTES_ROUTES) {
    const src = rustSrc(path)
    if (src === undefined) {
      failures.push({
        id: 'bytes-route-missing',
        what: `${path} is one of the two upload-bytes routes and was not found`,
        fix: [
          'If it moved or was renamed, update RUST_BYTES_ROUTES in scripts/check-invariants.mjs —',
          'a list that points at nothing guards nothing while reading as coverage. If the route',
          'was deleted, the upload-bytes path changed shape and this block needs to meet',
          'whatever replaced it.',
        ],
        found: [],
      })
    } else if (!/\bserve_upload\s*\(/.test(src)) {
      failures.push({
        id: 'bytes-route-bypasses-serveupload',
        what: `${path} serves upload bytes without going through serve_upload()`,
        fix: [
          'Put the decision back in the one place it lives: `serve_upload(bytes, mime, filename,',
          'cache)` from api/src/uploads.rs. A route that builds its own Response re-opens the P0',
          '— the uploader-declared MIME served inline, same-origin, and on the public route',
          'without even a session to blame.',
        ],
        found: [],
      })
    }
  }
  // And the decision itself must still be the allowlist in the one function:
  // INLINE_MIME gates the inline arm, nosniff rides every response. The
  // patterns are a census-style pointer, exact on purpose — a rename updates
  // them here; a real regression fails here.
  const serve = rustSrc(RUST_SERVE)
  if (serve === undefined) {
    failures.push({
      id: 'bytes-route-missing',
      what: `${RUST_SERVE} (the one serving function for upload bytes) was not found`,
      fix: [
        'If the helper moved or was renamed, update RUST_SERVE in scripts/check-invariants.mjs.',
        'The inline/download decision for upload bytes must stay in exactly one function with',
        'the INLINE_MIME allowlist and nosniff — that single point is the entire fix the P0 got.',
      ],
      found: [],
    })
  } else if (!/const INLINE_MIME/.test(serve) || !/nosniff/.test(serve)) {
    failures.push({
      id: 'bytes-route-bypasses-serveupload',
      what: `${RUST_SERVE} no longer visibly gates inline by the INLINE_MIME allowlist with nosniff`,
      fix: [
        'The inline/download decision for upload bytes is one allowlist (INLINE_MIME: raster +',
        'PDF inline, everything else attachment) and nosniff on every response. If a rename or',
        'refactor changed the spelling, update the two patterns in check-invariants.mjs — do',
        'not widen them to make a real regression pass.',
      ],
      found: [],
    })
  }
}

// ── Report ───────────────────────────────────────────────────────────────────

const BAR = '─'.repeat(78)
if (failures.length) {
  for (const f of failures) {
    console.error(`\n${BAR}\nFAIL  ${f.id}\n${BAR}`)
    console.error(`  ${f.what}\n`)
    for (const hit of f.found) console.error(`    ${hit.path}:${hit.line}\n      ${hit.text}`)
    if (f.found.length) console.error('')
    console.error('  WHAT TO DO INSTEAD:')
    for (const line of f.fix) console.error(`    ${line}`)
  }
  console.error(
    `\n${BAR}\n${failures.length} invariant check(s) failed. These are not style rules — each one is a\n` +
      'bug that has already shipped at least once. If you believe a match is a false\n' +
      'positive, say so in the PR; do not widen the pattern to make it pass.\n',
  )
  process.exit(1)
}

console.log(`invariants: ${sources.size} files scanned, ${RULES.length + CENSUS.length} rules, all clean`)
for (const n of notes) console.log(`  note: ${n}`)
