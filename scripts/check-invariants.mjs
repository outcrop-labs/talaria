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

/** "Everyone who is an admin", however it is spelled at a call site: the
 *  resolver's own function, or a local already holding its result.
 *
 *  The word boundary belongs to the IDENTIFIER alternatives only. Hung on the
 *  end of the whole group it silently un-matched every `adminUserIds()` form —
 *  `)` is not a word character, so `\b` there demands a transition that never
 *  arrives — which is to say the rule passed on the exact line it was written
 *  to catch. Found by the injection test, not by reading it. */
const ADMIN_LIST = String.raw`(?:await\s+)?(?:[\w$]+\s*\.\s*)?(?:adminUserIds\s*\(\s*\)|(?:admins|adminIds|adminUsers|allAdmins)\b)`

/** A hand-rolled audience: some named people, falling back to every admin.
 *
 *  Both orders, because both were written:
 *    owners.length ? owners : await adminUserIds()      judge.ts, verbatim
 *    !owners.length ? admins : owners                   the same thing, inverted
 *  and the `> 0` / `!== 0` / `=== 0` spellings, and a spread in either branch. */
// An optional `[ ...` before the identifier and `]` after it, because the
// spelling you actually get when a caller is BUILDING the audience rather than
// picking one is `owners.length ? [...owners] : [...admins]` — and the first
// version of this regex put `(?:\.\.\.)?` immediately before the identifier, so
// the `[` slipped past it. Same class of miss as the trailing `\b` above:
// written for the line already in the tree, blind to the next one.
const OPEN = String.raw`(?:\[\s*)?(?:\.\.\.)?\s*`
const CLOSE = String.raw`\s*\]?`
const HAND_ROLLED_AUDIENCE = new RegExp(
  String.raw`\b([A-Za-z_$][\w$]*)\s*\.\s*length\s*(?:>\s*0\s*|!==?\s*0\s*)?\?\s*${OPEN}\1${CLOSE}\s*:\s*${OPEN}${ADMIN_LIST}` +
    String.raw`|!?\s*\b([A-Za-z_$][\w$]*)\s*\.\s*length\s*(?:===?\s*0\s*)?\?\s*${OPEN}${ADMIN_LIST}${CLOSE}\s*:\s*${OPEN}\2\b`,
  'g',
)

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
    id: 'second-mail-gate-definition',
    pattern:
      /(?:^|\n)\s*(?:export\s+)?(?:async\s+)?function\s+sendGatedMail\b|(?:^|\n)\s*(?:export\s+)?(?:const|let|var)\s+sendGatedMail\s*[:=]/g,
    allow: ['ui/src/server/notifications.ts'],
    // Also asserted to exist exactly once — see the post-check below.
    what: 'a second definition of `sendGatedMail`',
    fix: [
      "Import it: `import { sendGatedMail } from '@/server/notifications'` (or `./notifications`).",
      'It answers ONE question — "is this deployment allowed to mail anybody at all?" — and it is',
      'the only place that asks it before a send. The reason it is a function and not a flag you',
      'check yourself is the bug it was extracted from: `addNotification` consulted the switch and',
      '`runDigest` did not, so an admin who turned email OFF still had a daily digest mailed to',
      'every user in the workspace. The control was named, audited, and inert.',
      'A second definition, or a second hand-written `if (delivery.emailEnabled)`, is that bug',
      'again with a different subject.',
    ],
  },
  {
    id: 'hand-rolled-audience',
    pattern: HAND_ROLLED_AUDIENCE,
    // No `allow`. There is no file in which this expression is the right answer,
    // including the one that owns the resolver.
    what: 'an audience worked out by hand, falling back to every admin',
    fix: [
      "Ask the resolver: `import { audienceFor } from '@/server/approvals'` (or",
      "`const { audienceFor } = await import('./approvals')` where the cycle demands it), then",
      '`const who = await audienceFor(<authority>)` and address `who.content`.',
      '',
      'THE AUTHORITY IS THE POINT. `{ by: "board", boardId }` for anything a board decides,',
      '`{ by: "user", userIds }` for one person\'s own business, `{ by: "admin" }` for org-scoped',
      'things, `{ by: "admin", onBoard }` for admin work whose TEXT quotes one board, and',
      '`{ by: "nobody" }` when no route in the product can act on it. Declaring which one is the',
      'question this expression skips.',
      '',
      'THIS EXACT LINE SHIPPED. `judge.ts` resolved a QA escalation audience with',
      '`owners.length ? owners : await adminUserIds()` and it was wrong in BOTH directions at',
      'once. On an unassigned ticket it sent the ticket TITLE and the judge\'s issue list to every',
      'org admin — including admins with no membership of that board, the disclosure the approval',
      'escalation had just closed, through a different door. And because the fallback went to the',
      'admins instead of the BOARD, the board\'s own editors — the only people who can approve the',
      'ticket, ask for changes or close it — were never told at all. Its doc comment claimed it',
      'was "the same rule" as the approvals path and named a function that does not exist.',
      '',
      '"Nobody is named, so tell the admins" is almost never the rule. The people who can ACT are.',
      'When genuinely nobody can act, that is `who.fact` — the admins, told THAT something is',
      'stuck and not what it says, so they can fix the access rather than read the content.',
    ],
  },
  {
    id: 'second-audience-resolver-definition',
    pattern:
      /(?:^|\n)\s*(?:export\s+)?(?:async\s+)?function\s+audienceFor\b|(?:^|\n)\s*(?:export\s+)?(?:const|let|var)\s+audienceFor\s*[:=]/g,
    allow: ['ui/src/server/approvals.ts'],
    // Also asserted to exist EXACTLY once — see the post-check below. That
    // companion is not optional: when `closedToAgents` was renamed, its
    // duplicate rule kept passing on a tree that no longer contained it.
    exactlyOnce: true,
    what: 'a second definition of `audienceFor`',
    fix: [
      "Import it: `import { audienceFor } from '@/server/approvals'`.",
      'It answers ONE question — "who may be told about this thing, and how much of it?" — and it',
      'answers the whole of it: the ids, AND the split between the people who may be told what the',
      'thing IS (`content`) and the people who may only be told THAT it exists so they can unblock',
      'it (`fact`). A second definition answers half and picks its own audience for the rest,',
      'which is the shape of every disclosure bug in this file\'s history.',
      '',
      'Three copies of this question existed before it was one: `approvalAudience` here,',
      '`owners.length ? owners : adminUserIds()` in judge.ts, and a raw',
      "`select id from users where role = 'admin'` in gaps.ts. Two of the three leaked.",
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
/** Census entries on `hand-written-harness` that are NOT debt: a pass-through
 *  proxy and the live persona-conversation paths. They stay on the census
 *  (their counts are still exact, so a new call in one of them fails) but they
 *  are excluded from the "still owed" note, because they are never going to be
 *  ported and a debt figure that never reaches zero is a figure nobody reads. */
const HARNESS_NOT_A_HARNESS = new Set([
  'ui/src/routes/api/llm.v1.chat.completions.ts',
  'ui/src/routes/api/chat.ts',
  'ui/src/server/chat-persist.ts',
  'ui/src/server/channel-replies.ts',
])

/** THE THIRD CATEGORY IS GONE, and the deletion is the finding.
 *
 *  This set used to name five files — muse.ts, briefing.ts, outreach.ts,
 *  work-dispatch.ts and plan-persona-turn.ts — that WERE declared harnesses but
 *  supplied their own transport, because `runHarness` could not pass a model's
 *  own tools through, stream, carry ledger attribution, or route a persona TIER
 *  id. Four independent agents hit that one gap and each wrote the same shim.
 *
 *  run.ts serves all four now (`def.tools`, `runHarnessStreamed`, `ctx.ledger`,
 *  `ctx.tier`), so the five shims were DELETED rather than reduced —
 *  plan-persona-turn.ts as a whole file. The census below is back to what a
 *  census should be: the permanent exceptions and nothing else. Anything that
 *  reaches a model by hand from here on is a regression, not debt, and it fails
 *  this rule with no entry to hide behind.
 *
 *  This comment is the only trace kept on purpose, so the next author who needs
 *  one of those four capabilities looks for the runner slot instead of writing
 *  a sixth shim. */

const CENSUS = [
  {
    id: 'hand-written-harness',
    // Reaching a model directly: the four transports. `server/gateway.ts` and
    // `server/llm-gateway.ts` DEFINE them and are excluded by path below.
    pattern: /\b(?:proxyChat|completeViaGateway|buildUpstream|fetchUpstream)\(/g,
    // The transports themselves, the runner that is the one legitimate caller of
    // all four, and tests that drive a transport on purpose.
    exempt: (path) =>
      path === 'ui/src/server/gateway.ts' ||
      path === 'ui/src/server/llm-gateway.ts' ||
      path.startsWith('ui/src/server/harness/') ||
      path.endsWith('.test.ts'),
    what: 'a model call written by hand instead of declared as a harness',
    fix: [
      'Declare it: `defineHarness({ ... })` in ui/src/server/harness/ and call it through',
      '`runHarness`. The runner owns model resolution, the capability floor, structured-output',
      'parsing WITH a repair round-trip, the guardrail pass, metering, and the harness_runs row.',
      'A hand-written call gets none of those, and gets them wrong in its own particular way:',
      'the audit found six different JSON extractors, six copies of the model-fallback chain,',
      'and three model paths reaching users with no guardrail at all — every one of them a call',
      'site that was written by hand by somebody who had read the code around it.',
      '',
      'THE PORT IS DONE AND THE CENSUS IS AT ITS FLOOR: the four entries below are a',
      'pass-through proxy and three live persona conversations, none of which has a prompt, a',
      'schema or a model policy to declare. There is no debt column any more, so a new match',
      'here is not "one more to port" — it is a call that went around the runner, and the fix',
      'is to declare it rather than to add a line to this table.',
      '',
      'IF WHAT YOU NEED IS A RUNNER CAPABILITY, ASK FOR IT IN run.ts. The five shims this',
      'census used to carry (muse.ts, briefing.ts, outreach.ts, work-dispatch.ts and the whole',
      'of plan-persona-turn.ts) all existed for four missing slots, and all four exist now:',
      "  a model's OWN tools     `tools: 'own'` on the definition (harness/transport.ts ToolPolicy)",
      '  streaming to a screen   `runHarnessStreamed(def, input, ctx, { stream, onDelta })`',
      '  ledger attribution      `ctx.ledger` — source / refId / taskId',
      '  a persona TIER id       `ctx.tier` — the alias NAME; the runner assembles `<agent>-<alias>`',
    ],
    sites: {
      // The public gateway route is a PASS-THROUGH proxy: it relays a caller's
      // own body to their chosen model and streams the answer back. It has no
      // prompt, no schema and no model policy of its own to declare, and it
      // already runs `guardCompletion` on all four of its exit paths.
      'ui/src/routes/api/llm.v1.chat.completions.ts': 2,
      // Live persona conversation. A human is talking to an agent and the reply
      // streams to their screen token by token; there is no structured contract
      // to parse and no repair turn that would make sense mid-stream. These
      // guard through `guardChatReply`, which is the right shape for a path that
      // sees tool NAMES but not tool results.
      //
      // These are NOT the streaming exception `runHarnessStreamed` closed. That
      // one is for a harness — a declared prompt and a declared contract —
      // whose OUTPUT happens to arrive token by token (the Muse's prose kinds,
      // the briefing chat-back), and both moved onto the runner. A chat turn has
      // no prompt of Talaria's to declare: the messages are the human's.
      'ui/src/routes/api/chat.ts': 1,
      'ui/src/server/chat-persist.ts': 1,
      'ui/src/server/channel-replies.ts': 1,
    },
  },
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
      // The four board-view copies this census used to carry are GONE, not
      // renamed: FilterBar and TaskDetail spread `OFF_BOARD_STATUSES`, and the
      // two byte-identical closed predicates (field-pills + Board.svelte)
      // collapsed into `isClosedStatus`, which Board.svelte now imports.
      // The census shrank to the two definitions, which is what a census is for.
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
    id: 'raw-transport-send',
    // A direct call to the mail transport. Legal in four places and nowhere
    // else — anything that mails a person because of a NOTIFICATION or a DIGEST
    // must go through `sendGatedMail`, which is where the instance master
    // switch is asked.
    pattern: /\bsendEmail\s*\(/g,
    what: 'a direct call to the mail transport, bypassing the instance master switch',
    fix: [
      "Send through the gate: `import { sendGatedMail } from '@/server/notifications'`. It asks the",
      'instance-wide email switch FRESH on every mail, attaches `List-Unsubscribe`, and tells you',
      'whether the send was refused (`blocked`) rather than failed — the two are different and the',
      'breaker, the digest retry and /observability all care which one happened.',
      '',
      'THIS RULE EXISTS BECAUSE THE SWITCH WAS ALREADY BYPASSED ONCE. `addNotification` consulted',
      '`getNotifyDelivery` and the daily digest did not, so an admin who switched email OFF still',
      'had a digest mailed to every user in the workspace, every morning, from a control that is',
      'named "email delivery" and writes an audit row when you flip it. Nothing failed; the second',
      'sender simply never asked. A second `if (emailEnabled)` next to the send would have been the',
      'same bug waiting for a third sender, which is why the answer is one function and this check.',
      '',
      'The four sites below are the complete list of code allowed to touch the transport:',
      '  server/email.ts        the definition',
      '  server/notifications.ts  `sendGatedMail` — the gate itself, and the ONLY gated path',
      '  server/invites.ts      an invitation a human just typed an address into. Not a',
      '                         notification and not governed by the notification switch: it is a',
      '                         direct reply to an admin action, and gating it would silently break',
      '                         invites on every instance that has not turned notification mail on',
      '                         (which is all of them — it defaults to off).',
      '  routes/api/admin.email.ts  the admin\'s own test send, which is how you verify a provider',
      '                         BEFORE turning delivery on. Gating it would make the switch',
      '                         untestable until after it was flipped.',
      'A fifth is a bypass. If you believe you need one, say so in the PR.',
    ],
    sites: {
      'ui/src/server/email.ts': 1,
      'ui/src/server/notifications.ts': 1,
      'ui/src/server/invites.ts': 1,
      'ui/src/routes/api/admin.email.ts': 1,
    },
  },
  {
    id: 'admin-list-outside-the-resolver',
    // Both spellings of "fetch every admin": the resolver's own function, and
    // the raw query it wraps. gaps.ts used the second one to fan an agent's
    // free text to the whole admin list, so matching only the first would have
    // watched one door.
    // `role in ('admin')` is the same query with different punctuation, and was
    // the spelling this rule did not watch. Comparison operator OR an `in` list.
    pattern:
      /\badminUserIds\s*\(|(?:\bwhere|\band)\s+(?:[\w$]+\s*\.\s*)?role\s*(?:=\s*'admin'|in\s*\([^)]*'admin'[^)]*\))/g,
    what: 'the admin list, fetched outside the one audience resolver',
    fix: [
      "Ask `audienceFor(<authority>)` in server/approvals.ts and address `who.content` — or",
      '`who.fact` when nobody can act and the admins are being asked to FIX that rather than to',
      'read what is stuck. The admin list is an answer to "who may be told"; fetching it yourself',
      'is deciding that question at the call site, which is where every leak so far was written.',
      '',
      'BEING AN ADMIN IS NOT A READ GRANT. It is a set of powers over the workspace, and none of',
      'them is "may be shown a board you were never added to". An admin cannot open the board, the',
      'ticket or the plan; sending them the title of one is disclosure with no action attached to',
      'it. `{ by: "admin", onBoard }` exists for the middle case — admin work whose TEXT quotes one',
      'board — and gives the content to the admins on that board and the FACT to the rest.',
      '',
      'THE FACT IS PART OF THE ANSWER, NOT A SECOND QUESTION. `audienceFor` returns `content` AND',
      '`fact` from one resolution, and a caller that addresses one of them by asking the resolver',
      'and the other by fetching the admin list has two answers to one question. That is what the',
      'SLA stall report did — `adminUserIds()` for approvals whose own authority had something',
      'narrower to say — and it is why `content: []` could mean "announced to nobody" in one file',
      'and "reported to every admin" in another, about the same row.',
      '',
      'The sites below are the complete list of code allowed to reach for it:',
      '  server/approvals.ts  the definition, and the resolver that consumes it — including the',
      '                       FACT half, which is why nothing downstream needs the list any more.',
      '  server/users.ts      NOT an audience: "which elevated delegations belong to an admin".',
      'Anything else on this census is debt, and the note printed on every run names it.',
    ],
    sites: {
      'ui/src/server/approvals.ts': 3, // adminUserIds() itself (call + query) and the resolver's use
      'ui/src/server/users.ts': 1, // `and u.role = 'admin'` — a delegation question, not an audience
      // ── Known debt, owned by a later round ──────────────────────────────────
      // EMPTY, and both departures are the same fix rather than two:
      //   agent.problem.ts    takes an authorised `taskId`, asks `agentTextAuthority` for the
      //                       authority and `audienceFor` for the people.
      //   workbench-mcp.ts    `request_repo` no longer announces itself at all. It calls
      //                       `announceApproval('repo_request:<id>')`, the same announcer the
      //                       census's own authority feeds, so the row has ONE announcement path.
      //   digest.ts           the stall report addresses `census.audience.get(key).fact`.
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
  ['second-mail-gate-definition', 'sendGatedMail'],
  ['second-audience-resolver-definition', 'audienceFor'],
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
  // EXACTLY once, for the rules that ask for it. `allow` exempts the home file
  // from the duplicate rule entirely, so a second definition INSIDE that file is
  // the one copy neither half of this check would otherwise see — and for a
  // resolver whose whole value is that there is one of it, two in one file is
  // the same bug as two in two files.
  if (rule.exactlyOnce && n > 1) {
    failures.push({
      id: `${ruleId}-defined-twice-at-home`,
      what: `${name} is defined ${n} times in ${home}; there must be exactly one`,
      fix: [
        'Delete all but one. The point of this function is that there is a single answer to its',
        'question — a second definition in its own file is not an exception to that, it is the',
        'duplication with a shorter import path.',
      ],
      found: [],
    })
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
  // "All clean" on this rule means "nobody added a NEW hand-written model call".
  // That is now the whole statement, because the census is at its floor — but it
  // is not the statement a reader assumes, so say the floor out loud and PROVE it
  // from the table rather than asserting it in prose.
  //
  // DERIVED from the census, never written by hand, for the reason the last two
  // rounds learned the hard way: the previous version of this block described a
  // debt column and five shim files, and kept printing that description for the
  // whole of the round that deleted them. A note that can only describe the
  // census it is computed from cannot outlive its subject.
  if (rule.id === 'hand-written-harness') {
    const owed = Object.entries(rule.sites).filter(([p]) => !HARNESS_NOT_A_HARNESS.has(p))
    if (owed.length) {
      notes.push(
        `${owed.length} file(s) still reach a model by hand instead of declaring a harness ` +
          `(${owed.reduce((s, [, n]) => s + n, 0)} call sites: ${owed.map(([p]) => p.replace(/^ui\/src\//, '')).join(', ')}). ` +
          'Each one re-implements some of model resolution, structured-output parsing, the repair ' +
          'turn, the guardrail pass and metering, and gets a different subset of them wrong. On the ' +
          'census in scripts/check-invariants.mjs.',
      )
    } else {
      // THE CENSUS IS AT ITS FLOOR. Said out loud, once, because "no failures" is
      // not the same statement and this is the one somebody will want to cite.
      notes.push(
        'THE HARNESS PORT IS COMPLETE AND THIS CENSUS IS AT ITS FLOOR: every entry is a permanent ' +
          'exception (one pass-through proxy, three live persona conversations) and nothing on it is ' +
          'debt. No file in the tree reaches a model with a hand-written prompt, parser, fallback ' +
          'chain and guard pass, and none supplies its own transport to work around a missing runner ' +
          'capability — run.ts serves tools, streaming, ledger attribution and tier routing itself. ' +
          'The next match on this rule is a regression, not a backlog item.',
      )
    }
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
  // Any admin-list site that is not one of the two DECLARED ones is a file still
  // addressing the admins as an audience, and a clean run must say so — or "all
  // clean" reads as "and nothing fans content to every admin any more".
  //
  // DERIVED FROM THE CENSUS, never written out by hand, because the hand-written
  // version is what this round had to correct: it named two files and asserted
  // they were "org-level by nature, so neither is the board leak that was just
  // closed", which stopped being true the moment `repo_request` was bounded to a
  // board — and it kept printing, on every clean run, the opposite of the code.
  // A note that cannot outlive its subject is the only kind worth printing: this
  // one disappears when the census does, and it cannot describe a file that is
  // no longer on it.
  if (rule.id === 'admin-list-outside-the-resolver') {
    const DECLARED = new Set(['ui/src/server/approvals.ts', 'ui/src/server/users.ts'])
    const owing = Object.keys(rule.sites).filter((p) => !DECLARED.has(p))
    if (owing.length) {
      notes.push(
        `${owing.join(', ')} still reach for the admin list instead of asking \`audienceFor\` — an ` +
          'audience decided at the call site, for a subject whose authority is declared elsewhere. On ' +
          'the census in scripts/check-invariants.mjs; owned by a later round.',
      )
    }
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

// EVERY TOOL AN AGENT CAN CALL MUST BE SIMULATED AND EXERCISED.
//
// THE RULE. A tool registered in `mcp/src/index.ts` is a verb Talaria hands to
// every fleet agent in the workspace. Three things must then be true of it, and
// this check fails the build when any of them is not:
//
//   MODELLED   it appears in fitness/toolbox/talaria-tools.ts with the real
//              description and the real argument names (the sync test holds it
//              to those; this holds it to existing at all)
//   BACKED     it has a handler in fitness/toolbox/sandbox.ts, so a model in the
//              eval sweep can actually call it against simulated state
//   EXERCISED  something drives it — a sandbox test, or a harness's
//              `dryRun.tools` surface. A backend nobody calls is a backend
//              nobody has checked.
//
// WHY IT IS AN INVARIANT AND NOT A CONVENTION. The toolkit reached forty-four
// tools while the simulator modelled sixteen. Nothing was wrong with any one
// commit — each new tool was reviewed on its own, and "add an eval" is the kind
// of follow-up that never has an owner. The result was that twenty-eight verbs
// an org depends on had no simulated backend and no fixture, and the model
// fitness page reported confident scores over the third of the surface that
// happened to be covered.
//
// THIS IS ALSO THE PATTERN WE ASK OF SDK AUTHORS. A plugin's harness is required
// to ship evals (docs/HARNESSES.md); a rule the platform exempts itself from is
// a rule nobody follows. So the platform's own toolkit is held to it first, in
// CI, with the worklist printed rather than described.
{
  const MCP = sources.get('mcp/src/index.ts') ?? ''
  const MODEL = sources.get('ui/src/server/fitness/toolbox/talaria-tools.ts') ?? ''
  const SANDBOX = sources.get('ui/src/server/fitness/toolbox/sandbox.ts') ?? ''

  const registered = [...MCP.matchAll(/server\.registerTool\(\s*'([a-z_]+)'/g)].map((m) => m[1])
  // The guard on the guard: a renamed `registerTool` would leave this rule
  // asserting over an empty list and passing forever.
  if (registered.length < 30) {
    failures.push({
      id: 'toolkit-coverage-cannot-read-the-toolkit',
      what: `found ${registered.length} tool registrations in mcp/src/index.ts, which cannot be right`,
      fix: [
        'The scanner looks for `server.registerTool(\'name\'`. If registration was renamed or wrapped,',
        'update the pattern in scripts/check-invariants.mjs — do NOT delete the rule. A coverage check',
        'that silently matches nothing is worse than no coverage check: it reports "all clean".',
      ],
      found: [],
    })
  }

  // Handlers only: the top-level `name: (args, world) =>` keys of the HANDLERS
  // object. Matching the whole file would count `research:` on the world
  // interface as a backend, which is exactly the false pass to avoid.
  const from = SANDBOX.indexOf('const HANDLERS')
  const to = SANDBOX.indexOf('export const backedToolNames')
  const backed = new Set([...SANDBOX.slice(from === -1 ? 0 : from, to === -1 ? undefined : to).matchAll(/^ {2}([a-z_]+): \(/gm)].map((m) => m[1]))

  // Anything that drives a tool by name: the sandbox's own tests, and the
  // `dryRun.tools` surfaces harnesses declare.
  const drivers = [...sources].filter(([p]) => p.startsWith('ui/src/server/fitness/toolbox/') || p.startsWith('ui/src/server/harness/defs/')).map(([, src]) => src)

  const unmodelled = registered.filter((n) => !new RegExp(`name: '${n}'`).test(MODEL))
  const unbacked = registered.filter((n) => !backed.has(n))
  const unexercised = registered.filter((n) => !drivers.some((src) => src.includes(`'${n}'`)))

  for (const [id, names, what, fix] of [
    [
      'mcp-tool-not-modelled',
      unmodelled,
      'a tool the fleet can call that the fitness suite does not model',
      [
        'Add it to ui/src/server/fitness/toolbox/talaria-tools.ts: the name, the DESCRIPTION COPIED',
        'VERBATIM from mcp/src/index.ts, the real argument names, and a `group`. The sync test',
        'compares all three against the real registration and will tell you which part drifted.',
      ],
    ],
    [
      'mcp-tool-not-simulated',
      unbacked,
      'a tool the fitness suite offers with no simulated backend behind it',
      [
        'Add a handler to HANDLERS in ui/src/server/fitness/toolbox/sandbox.ts, over the in-memory',
        'world. Simulate the REFUSALS as well as the happy path — "personal assistants only", "no',
        'Google account connected", "that ticket is off the table". A sandbox that only ever says yes',
        'measures nothing: the failures worth catching are a model reaching for a tool its identity',
        'does not carry, and a model narrating a result it was refused.',
      ],
    ],
    [
      'mcp-tool-never-exercised',
      unexercised,
      'a simulated tool that nothing ever calls',
      [
        'Drive it: a case in ui/src/server/fitness/toolbox/sandbox.test.ts asserting the rule that makes',
        'the tool worth simulating, and — where the tool is part of a job a model does — the harness',
        "surface that offers it (`dryRun.tools` on a definition in ui/src/server/harness/defs/).",
        '',
        'THIS IS THE STEP THAT IS ALWAYS SKIPPED, and it is the one that finds things. A backend written',
        'from the tool description and never called is a guess about production with a test-shaped',
        'wrapper around it.',
      ],
    ],
  ]) {
    if (!names.length) continue
    failures.push({ id, what, fix: [...fix, '', 'TOOLS:', ...names.map((n) => `  ${n}`)], found: [] })
  }

  if (!unmodelled.length && !unbacked.length && !unexercised.length) {
    notes.push(
      `all ${registered.length} tools in Talaria's MCP toolkit are modelled, simulated against in-memory state, and exercised ` +
        '(fitness/toolbox/). This is the coverage bar docs/HARNESSES.md asks of SDK plugin authors, held by the platform first.',
    )
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
