#!/usr/bin/env node
// Talaria invariant check — the tripwire under the predicates that keep getting
// re-implemented. Zero dependencies, plain Node, run it with `node
// scripts/check-invariants.mjs` from anywhere.
//
// WHY THIS FILE EXISTS
//   Seven consecutive rounds of review found the same shape of bug: a rule that
//   decides whether an AGENT may write to a ticket, or which statuses are
//   terminal, gets written out by hand at a new call site instead of imported.
//   Each round centralized one predicate; the next round found the neighbouring
//   copy that had not been. `closedToAgents` was defined exactly once with eight
//   importing call sites — and a verifier immediately found `ticketState`, a
//   fourth expression of the same question, in the one file that round did not
//   treat as a copy.
//
//   Round eight collapsed all four into `agentTicketRefusal`, which takes the
//   AGENT as a required argument so the board-policy half cannot be skipped, and
//   collapsed the three hand-written active-column lookups into `statusMeta`'s
//   `activeKey`. Both are rules below. Note what changed in this file when
//   `closedToAgents` was renamed: the duplicate rule kept PASSING on a tree that
//   no longer contained its subject. Only the exists-exactly-once post-check
//   caught it. Every single-definition rule needs that companion, or the rule
//   quietly becomes decoration.
//
//   Nothing failed for any of that. Every one of those copies was merged by a
//   human who had read the code around it. Centralizing the predicate an eighth
//   time without a check that FAILS on the ninth copy just schedules round nine.
//   This is that check.
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
 *  output, deps) is skipped by SKIP below. */
const SOURCE_DIRS = ['ui/src', 'mcp/src']
const SKIP_DIRS = new Set(['node_modules', 'dist', 'build', '.output', '.vinxi', '.tanstack'])
const SKIP_FILES = new Set(['routeTree.gen.ts'])
const EXTS = ['.ts', '.tsx', '.mts', '.js', '.mjs']

// ── Rules ────────────────────────────────────────────────────────────────────

/** Forbidden outright, except in the files that legitimately define the thing. */
const RULES = [
  {
    id: 'hand-rolled-agent-write-predicate',
    // `doneKeys.includes(k) || OFF_BOARD_STATUSES.includes(k)`, either order,
    // any receiver (`meta.doneKeys`, destructured `doneKeys`).
    pattern:
      /(?:\bdoneKeys\s*\.\s*includes\s*\([^)]*\)\s*\|\|\s*OFF_BOARD_STATUSES\s*\.\s*includes|\bOFF_BOARD_STATUSES\s*\.\s*includes\s*\([^)]*\)\s*\|\|\s*[\w.]*doneKeys\s*\.\s*includes)/g,
    allow: ['ui/src/server/statuses.ts'],
    what: 'the terminal-status predicate, written out by hand',
    fix: [
      'Call `meta.terminal(key)` — `statusMeta()` returns it for exactly this reason.',
      'The half-written version of this expression (the one that checks doneKeys and forgets',
      'OFF_BOARD_STATUSES) is how a pickup queue keyed `cancelled` became a dispatch target.',
      'It is a function and not a list on purpose: a function cannot be half-copied.',
    ],
  },
  {
    id: 'second-agentTicketRefusal-definition',
    pattern:
      /(?:^|\n)\s*(?:export\s+)?(?:async\s+)?function\s+agentTicketRefusal\b|(?:^|\n)\s*(?:export\s+)?(?:const|let|var)\s+agentTicketRefusal\s*[:=]/g,
    allow: ['ui/src/server/tasks.ts'],
    // Also asserted to exist exactly once — see the post-check below.
    what: 'a second definition of `agentTicketRefusal`',
    fix: [
      "Import it: `import { agentTicketRefusal } from '@/server/tasks'` (or",
      "`const { agentTicketRefusal } = await import('./tasks')` where the cycle demands it).",
      'It answers ONE question — "may this agent act on this ticket?" — and it answers the',
      'whole of it: the board\'s agent policy, ticket archival, board archival, and (for a',
      "write) closed status. Its predecessors each answered part. `closedToAgents` asked",
      'nothing about board policy, so revoking an agent\'s grant 403\'d every write route while',
      'the heartbeat kept serving the ticket. `ticketState` in work-dispatch.ts asked neither',
      'archival clause, so archiving a ticket did not stop the live work session on it.',
    ],
  },
  {
    id: 'hand-rolled-active-column-lookup',
    // `s.category === 'active'` outside statuses.ts. Three callers each spelled
    // `listStatuses(...).find(s => s.category === 'active')?.key` to answer
    // "where does a ticket go while it is being worked?" — and none of them
    // excluded terminal columns, so a column labelled "Cancelled" was a legal
    // answer to all three.
    pattern: /\.\s*category\s*===\s*'active'/g,
    allow: ['ui/src/server/statuses.ts'],
    what: "an active-column lookup, re-derived outside `statusMeta`",
    fix: [
      'Use `meta.activeKey` (the board\'s working column) or `meta.workingKeys` (is this ticket',
      'still in play?) from `statusMeta()`. Both are picked from the one `placeable` list, so',
      'they can never be a terminal column or the system Blocked column.',
      'The three copies of this expression were the dispatch prompt\'s "triage_ticket to status',
      "\"<hint>\" while you work\", the QA judge's revision bounce, and the human reviewer's",
      '"request changes". On a board whose first active column is labelled "Cancelled" — legal,',
      'and `agentStartConflict` does not refuse it — all three sent the ticket to a TERMINAL',
      'status, and the first one told the AGENT to do it.',
    ],
  },
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
]

/** Legal in named files, in a known quantity, and forbidden everywhere else.
 *  A census entry is a promise: "these are all of them, and a new one is a bug."
 *  Counts are exact — a file that GAINS an occurrence fails, and a file that
 *  LOSES its last one fails too, so the table cannot rot into a permanent
 *  amnesty for code that was fixed years ago. */
const CENSUS = [
  {
    id: 'off-board-status-literal',
    // 'failed' next to 'cancelled' (either order) in an array literal or a type
    // union — i.e. a copy of OFF_BOARD_STATUSES.
    pattern: /(?:(['"])failed\1\s*[,|]\s*(['"])cancelled\2|(['"])cancelled\3\s*[,|]\s*(['"])failed\4)/g,
    what: "the off-board status list, spelled out as a literal",
    fix: [
      "Import the list: `import { OFF_BOARD_STATUSES } from '@/lib/task-const'` (client and",
      "server may both import it; `server/` already imports from `@/lib`).",
      'This set decides which statuses exist but are never COLUMNS. A copy that drifts leaves',
      'tickets in a status no view draws — work that has silently disappeared off the board.',
    ],
    sites: {
      // The two definitions. `server/statuses.ts` still declares its own; the
      // cross-check below fails if the two lists ever disagree, and the fix that
      // retires both this line and that check is one import.
      'ui/src/lib/task-const.ts': 1,
      'ui/src/server/statuses.ts': 1,
      // ── Known debt, owned by a later round ──────────────────────────────────
      // Each of these is a real copy and each should become an import. They are
      // listed rather than fixed because the change that added this file was
      // scoped to the two board views; listing them means the next hand sees
      // them on every CI run instead of discovering them in round nine.
      'ui/src/components/board/filter-bar.tsx': 1, // status filter options
      'ui/src/components/board/field-pills.tsx': 1, // `['done','cancelled','failed']` — the CLIENT-side terminal predicate
      'ui/src/components/board/task-detail.tsx': 2, // MOVE list + status picker
      'ui/src/routes/_app/boards/$boardId.tsx': 1, // `['done','cancelled','failed']` again, same predicate, second copy
    },
  },
  {
    id: 'board-agent-policy-in-sql',
    // `allow_all_agents` inside a query — the board agent policy, expressed as
    // SQL instead of as `boardAllowsAgent`.
    pattern: /\ballow_all_agents\b/g,
    what: 'the board agent policy, re-expressed in SQL',
    fix: [
      'Prefer `boardAllowsAgent(boardId, agent, facts?)` — it is the one definition of "may this',
      'agent touch this board", and `agentTicketRefusal` builds the ticket-level answer on it.',
      '',
      'SET-SCOPING is the legitimate exception: a JOIN that filters MANY boards at once cannot',
      'call a per-row JS predicate without an N+1. Those are on the census below — and they',
      'have already drifted from the predicate they copy:',
      '',
      '  boards.ts `listBoardsForAgent`   filters `b.archived_at is null`   ✔ agrees',
      '  uploads.ts `canAccessUpload`     does NOT filter archived          ✘ diverges',
      '  retrieval/index.ts `activityScope` does NOT filter archived        ✘ diverges',
      '',
      'So an agent cannot GET a ticket on an archived board and cannot list the board, but can',
      'still fetch its attachments and retrieve its content into an answer. Fixing that means',
      'ONE shared SQL fragment (the shape `statusCategorySql` already uses in server/statuses.ts),',
      'not a third hand-written `and b.archived_at is null`. Owned by a later round; listed here',
      'so it is on every CI run instead of being rediscovered.',
    ],
    sites: {
      'ui/src/server/boards.ts': 3, // getBoardAgentConfig, setBoardAgentConfig, listBoardsForAgent
      'ui/src/server/db/pg.ts': 1, // the column DDL
      // ── Known divergence, owned by a later round (see `fix` above) ──────────
      'ui/src/server/uploads.ts': 1, // canAccessUpload — no archival clause
      'ui/src/server/retrieval/index.ts': 1, // activityScope — no archival clause
    },
  },
  {
    id: 'hand-rolled-done-check',
    // `doneKeys.includes(...)` on its own. Legal in two places; anywhere else it
    // is the HALF of the terminal predicate that forgets the off-board keys.
    pattern: /\bdoneKeys\s*\.\s*includes\s*\(/g,
    what: '`doneKeys.includes(...)` — half of the terminal predicate',
    fix: [
      'Call `meta.terminal(key)` unless you specifically mean "is this a done-CATEGORY column',
      'on this board" and NOT "is this terminal" — the two differ by exactly the off-board',
      'keys, and every place that has confused them so far was a bug.',
      'If you really mean the narrow question, add your file to the census in',
      'scripts/check-invariants.mjs with a comment saying which question you are asking.',
    ],
    sites: {
      'ui/src/server/statuses.ts': 1, // the definition of terminal()
      'ui/src/server/tasks.ts': 1, // completedAt: a done-CATEGORY question, not a terminal one
    },
  },
]

/** The off-board list is currently written twice. Until `server/statuses.ts`
 *  imports it from `@/lib/task-const`, CI is what keeps the copies identical.
 *  When that import lands this check finds no local definition and retires
 *  itself — it does not need to be deleted by hand. */
const OFF_BOARD_SOURCES = ['ui/src/lib/task-const.ts', 'ui/src/server/statuses.ts']

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

const SWALLOWS = [
  { re: /if\s*\(\s*!\s*[\w.]+\.ok\s*\)\s*return\s*(?:\[\s*\]|null|\{\s*\}|undefined)/g, note: 'if (!r.ok) return <empty>' },
  { re: /\.catch\s*\(\s*\(\s*\)\s*=>\s*\(?\s*(?:\[\s*\]|null|\{\s*\}|undefined)/g, note: '.catch(() => <empty>)' },
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

/** Read the array literal assigned to OFF_BOARD_STATUSES, or null if the file
 *  no longer declares one (i.e. it imports it — the goal state). */
function offBoardListIn(src) {
  const m = /OFF_BOARD_STATUSES\s*(?::[^=]+)?=\s*\[([^\]]*)\]/.exec(src)
  if (!m) return null
  return [...m[1].matchAll(/['"]([^'"]+)['"]/g)].map((x) => x[1])
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

// Each single-definition predicate must exist EXACTLY once. Without this,
// deleting (or renaming) the canonical definition would leave the duplicate rule
// above passing forever on a tree that has none — which is precisely what
// happened when `closedToAgents` became `agentTicketRefusal`: the rule kept
// passing, and only this post-check caught that its subject had vanished.
for (const [ruleId, name] of [
  ['second-agentTicketRefusal-definition', 'agentTicketRefusal'],
  ['hand-rolled-active-column-lookup', "the active-column lookup (`s.category === 'active'`)"],
]) {
  const rule = RULES.find((r) => r.id === ruleId)
  const home = rule.allow[0]
  const n = matches(sources.get(home) ?? '', rule.pattern).length
  if (n < 1) {
    failures.push({
      id: `${ruleId}-home-is-empty`,
      what: `${name} no longer appears in ${home}; the duplicate check above is now inert`,
      fix: [
        `If it moved, update \`allow\` for '${ruleId}' in scripts/check-invariants.mjs so the`,
        'duplicate check keeps pointing at its home. If it was deleted, every call site that',
        'imported it is about to hand-roll it again — which is the entire history this file',
        'exists to stop.',
      ],
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
  // The board-policy census carries a KNOWN DIVERGENCE, not just duplication —
  // say so on every run, or "all clean" reads as "and they agree".
  if (rule.id === 'board-agent-policy-in-sql') {
    notes.push(
      'ui/src/server/uploads.ts and ui/src/server/retrieval/index.ts scope agent access by board ' +
        'policy in SQL and do NOT filter archived boards, while boards.ts `listBoardsForAgent` does. ' +
        'An agent can still fetch attachments from, and retrieve content out of, a board it cannot ' +
        'otherwise see. On the census in scripts/check-invariants.mjs; owned by a later round.',
    )
  }
  // Anything on the census beyond the two real definitions is debt, not policy.
  const debt = Object.entries(rule.sites).filter(([p]) => !OFF_BOARD_SOURCES.includes(p))
  if (rule.id === 'off-board-status-literal' && debt.length) {
    notes.push(
      `${debt.length} file(s) still spell out the off-board status list instead of importing it ` +
        `(${debt.reduce((s, [, n]) => s + n, 0)} occurrences). They are on the census in ` +
        'scripts/check-invariants.mjs, so they cannot multiply — but they should shrink to zero.',
    )
  }
}

// The two OFF_BOARD_STATUSES declarations must agree, until there is only one.
{
  const lists = OFF_BOARD_SOURCES.map((p) => [p, offBoardListIn(sources.get(p) ?? '')]).filter(([, l]) => l !== null)
  const [first, ...rest] = lists
  const disagree = rest.filter(([, l]) => l.join(',') !== first?.[1].join(','))
  if (first && disagree.length) {
    failures.push({
      id: 'off-board-list-copies-disagree',
      what: 'the two OFF_BOARD_STATUSES declarations have drifted apart',
      fix: [
        `${first[0]} says [${first[1].join(', ')}]`,
        ...disagree.map(([p, l]) => `${p} says [${l.join(', ')}]`),
        '',
        "Make `server/statuses.ts` import the list: `import { OFF_BOARD_STATUSES } from '@/lib/task-const'`.",
        'That is the fix for the drift AND for this check, which retires itself once there is',
        'only one declaration left to find.',
      ],
      found: [],
    })
  }
  if (lists.length === 1) notes.push('OFF_BOARD_STATUSES is now declared once — the cross-check found nothing to compare and is inert.')
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
