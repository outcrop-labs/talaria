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

import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs'
import {
  loadAuthority,
  unclassified as unclassifiedHermes,
  declaredSignpostNames,
  replacedPathsNotCatalogued,
} from './hermes-skill-authority.mjs'
import { join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { scanWebviewThreading, selfTest as desktopThreadingSelfTest } from './desktop-threading.mjs'
import { checkDropGuard } from './desktop-drop-guard.mjs'
import { checkReloadKeys } from './desktop-reload-keys.mjs'
import { checkOsLinks } from './desktop-os-links.mjs'
import { checkReleaseLatest, selfTest as releaseLatestSelfTest } from './release-latest.mjs'

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
  {
    id: 'ts-event-source-outside-the-door',
    pattern: /new EventSource\(/,
    allow: ['ui/src/lib/sse.ts'], // the one door
    what: 'an EventSource opened outside the one SSE door',
    fix: [
      'Use `openStream(url, (data) => { … })` from `@/lib/sse` — it returns the',
      'unsubscribe, so the effect teardown that used to say `es.close()` says `return',
      'openStream(…)`.',
      '',
      'WHY: four surfaces each opened their own stream and each decided differently what',
      'a frame that does not parse means — two swallow it, one lets JSON.parse throw',
      'inside the listener (an unhandled error, stream still open), one returns early.',
      'The door hands over the RAW data and lets each caller parse, because that judgment',
      'is per-surface; what it owns is the connection and its teardown.',
    ],
  },
  {
    id: 'ts-maybe-getter-copy',
    pattern: /type MaybeGetter</,
    allow: ['ui/src/lib/reactive-arg.ts'], // the one declaration
    what: 'a local `MaybeGetter` instead of the shared one',
    fix: [
      "Import it: `import { resolve, type MaybeGetter } from '@/lib/reactive-arg'`.",
      '',
      'WHY: sixteen files declared this type and then re-implemented the two-line',
      'resolution beside it (and two spelled the resolution differently again,',
      '`resolveModel` and `resolveValue`). A hook that takes `MaybeGetter<T>` and forgets',
      'to accept the eager form is a papercut at every call site; one declaration means',
      'one answer.',
    ],
  },
  {
    id: 'rust-hand-wrapped-gate',
    lang: 'rust',
    // The gate helper answers `Result<_, Response>`; a handler that unwraps it
    // by hand is a handler that has not taken the handler shape.
    pattern: /Err\((?:gate|resp)\)\s*=>\s*return (?:gate|resp),/,
    what: 'a gate result unwrapped by hand instead of with `?`',
    fix: [
      'Handlers answer `Result<Response, Response>` (axum implements `Handler` for any',
      '`R: IntoResponse`, and `Result<Response, Response>` is one), so the gate propagates:',
      '',
      '    let user = require_user(&state, &headers).await?;',
      '',
      'Every guard — `require_user`, `require_perm`, `require_admin`, `require_view`,',
      '`agent_caller` — already returns `Result<_, Response>`. ~360 call sites spelled the',
      'match out by hand, which is why the refusal envelope drifted between them.',
    ],
  },
  {
    id: 'rust-secretbox-unwrap',
    lang: 'rust',
    // The house unwrap: the secretbox or the logged 500. `state.secretbox()`
    // is the one call every key-touching route makes; a route that spells the
    // match out again is route 27. (Routes whose refusal is their OWN — a 400,
    // a page, a `catch(e)` — keep their own three lines and are not matched.)
    pattern: /let sb = match state\.secretbox\(\)\.await \{[\s\S]{0,80}?Err\(e\) => return (?:Ok\()?internal\(/,
    what: 'the secretbox unwrapped by hand instead of through `secretbox_or_500`',
    fix: [
      'Write `let sb = secretbox_or_500(&state, "<context>").await?;` — `talaria-session`',
      "owns the one conversion (it is the layer that may name `AppState` AND answer a",
      '`Response`; `talaria-error` cannot, because `AppState` depends on it).',
      '',
      'A route whose failure answer is its OWN — a 400 with the route\'s sentence, an OAuth',
      'callback page, a `catch(e)` that flattens to an operator message — keeps its own',
      'match; the rule only matches the house 500.',
    ],
  },
  {
    id: 'rust-as-object-unwrap',
    lang: 'rust',
    // The hand-wrapped 400 for a non-object body.
    pattern: /match as_object\(&/,
    allow: ['api/crates/talaria-error/src/lib.rs'], // object_or_400 IS the conversion
    what: 'the non-object-body 400 unwrapped by hand',
    fix: [
      'Write `let obj = object_or_400(&parsed)?;` — `talaria-error` owns the one conversion',
      "from `talaria_body`'s message to the 400 that carries it. ~194 route files spelled the",
      'match out by hand.',
      '',
      'Keep `talaria_body` pure: it answers the message and knows nothing about HTTP, which',
      'is why the conversion lives beside the envelope it produces.',
    ],
  },
  {
    id: 'rust-trap-block',
    lang: 'rust',
    // The hand-written pair, in every spelling it grew in: `return
    // thrown_internal_error();`, a bare tail expression, `Err(…)`, `Some(…)`,
    // the fully-qualified path. All of them were one concept written out ~860
    // times; `internal(context, e)` is that concept, once.
    pattern:
      /tracing::error!\([^;\n]*\);\s*\n\s*(?:return )?(?:Err\(|Some\()?(?:talaria_error::)?thrown_internal_error\(\)/,
    allow: ['api/crates/talaria-error/src/lib.rs'], // the envelope defines it
    what: 'a hand-written "log then answer the house 500" pair',
    fix: [
      'Write `return talaria_error::internal("<context>", e)` (or `Err(internal(…))`,',
      '`Some(internal(…))`, `internal(…)` as a tail expression) — `internal` IS that pair,',
      'defined once in api/crates/talaria-error/src/lib.rs: it logs `"{context}: {e}"` and',
      'returns the same byte-exact `thrown_internal_error()` response.',
      '',
      'WHY THIS RULE: the pair was written out by hand at ~860 call sites, and because every',
      'site re-made the decision, about a quarter of them re-made it as "no log at all" — a',
      '500 with nothing in the log and no way to tell which read failed. One home, one',
      'decision.',
    ],
  },
  {
    id: 'rust-thrown-internal-error-outside-its-envelope',
    lang: 'rust',
    // The umbrella behind the rule above: `thrown_internal_error` is the house
    // 500's BODY, and the only code that should reach for it directly is the
    // one function that answers it. Anywhere else it is either a hand-written
    // pair (caught above, with the tailored message) or a 500 nobody logged.
    pattern: /thrown_internal_error\(/,
    allow: ['api/crates/talaria-error/src/lib.rs'],
    what: 'a direct call to the house 500 envelope from outside talaria-error',
    fix: [
      'Use `talaria_error::internal("<context>", e)` — it logs and answers the house 500 in',
      'one call. If the failure has no error value to log (a discarded `.is_err()`, an absent',
      'config), pass the reason as the second argument: those sites keep the sentence they',
      'used to log, and gain the log line they did not have.',
      '',
      'The envelope itself stays in api/crates/talaria-error/src/lib.rs because its bytes are',
      'a client contract; no other file needs to name it.',
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
 *  one runner (harness/run.ts) and its transports (harness/transport.ts), which
 *  call the Rust relay on the app's gateway key — the TS tier holds no provider
 *  key and reaches no model any other way. */

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
    // safeFetch), excluded below. The SDK is NOT a second door: since the
    // client-parity pass its `api()` and `appApi()` are built on these verbs.
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
      'The counted file below is the door itself, at its exact call count — if the door is renamed',
      'or emptied, the stale-census half of this check fails instead of the rule going quietly',
      'inert. A fetch anywhere else in client code is a new stanza — the published SDK included:',
      'it used to carry its own weaker copy (no status, no deadline, no streams) on the belief that',
      "it could not import the app's door, and it can. If you believe you need one, say so in the PR.",
    ],
    exempt: (path) =>
      path.startsWith('ui/src/server/') ||
      path.startsWith('ui/src/routes/api/') ||
      path === 'ui/src/sw.ts' || // the service worker, like the SDK, is its own door: a classic script
      // with no module graph, it cannot import the app's — and it must stay import-free
      path.endsWith('.test.ts') ||
      !path.startsWith('ui/src/'), // the mcp/ and cli/ trees are not the browser app
    sites: {
      'ui/src/lib/fetch-json.ts': 8, // the door: getJson, getJsonOr404, getJsonOr, getText, getBlob, sendJson, postStream, getStream
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
    ],
    sites: {
      'ui/src/lib/fetch-json.ts': 1, // the door: the one SAME_ORIGIN RequestInit every verb spreads
    },
  },
]

// ── Machinery ────────────────────────────────────────────────────────────────

function walk(dir, out = [], exts = EXTS) {
  let entries
  try {
    entries = readdirSync(dir)
  } catch {
    return out
  }
  for (const name of entries) {
    const full = join(dir, name)
    if (statSync(full).isDirectory()) {
      if (!SKIP_DIRS.has(name)) walk(full, out, exts)
    } else if (exts.some((e) => name.endsWith(e)) && !SKIP_FILES.has(name)) {
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

// ─ Duplicate function bodies ────────────────────────────────────────────────
//
// THE LESSON THIS ENCODES. Commit 9d4ed398 collapsed 47 copies of the epoch
// helper `now_ms` onto one home in talaria-agent-auth. Seven byte-identical
// copies grew back within the week, in crates added after the collapse, and
// nothing failed — because nothing was watching. A dedupe that is not
// accompanied by a check that FAILS on the next copy is a chore, not a fix:
// the tree drifts back to where it was, one plausible-looking local helper at
// a time, and the next reader re-does the work.
//
// HOW IT JUDGES. A function is a candidate when its body, whitespace
// normalized, is long enough to be more than an accessor or a one-line
// delegate (120 chars of Rust, 100 of TS), and the match is by name AND body,
// so two same-named functions that do different things are not a cluster.
// Whole bodies only: this deliberately does not look inside a function for a
// repeated block, which is where a rule like this starts producing arguments
// instead of fixable hits.
//
// TEST MODULES ARE OUT OF SCOPE BY CONSTRUCTION. Per-crate `#[cfg(test)]`
// fixtures are near-identical on purpose (each crate builds its own tiny
// world), and including them buries the real clusters under hundreds of
// false positives. The ranges are precomputed by brace-matching the module
// body so a `fn` inside one never reaches the report.
//
// WHY AN ALLOW LIST AND NOT A COUNT. Some pairs are genuinely two answers to
// different questions that happen to coincide today (a `role == "admin"`
// gate beside a `role != "member"` one). Those need a sentence, not a
// pattern: `allow` entries carry `paths` (the exact set, order-insensitive)
// and a `why`. Each one is checked for staleness — the moment its paths stop
// forming a cluster, the entry fails and asks to be deleted, which is the
// signal that a wave finished its share of the worklist.

/** The body text between the brace at/after `start` and its match. Rust and
 *  TS format strings keep `{}` balanced, so depth counting survives the
 *  common case; a lone brace inside a string is rare enough to accept. */
function braceBody(src, start) {
  const open = src.indexOf('{', start)
  if (open === -1) return null
  let depth = 0
  for (let i = open; i < src.length; i++) {
    const c = src[i]
    if (c === '{') depth++
    else if (c === '}' && --depth === 0) return [open + 1, i]
  }
  return null
}

/** Ranges of `#[cfg(test)] mod … { … }` bodies, so test fixtures stay out. */
function cfgTestRanges(src) {
  const ranges = []
  const re = /#\[cfg\(test\)\]/g
  let m
  while ((m = re.exec(src)) !== null) {
    if (!/\bmod\s+[A-Za-z_][A-Za-z0-9_]*\s*\{/.test(src.slice(m.index, m.index + 200))) continue
    const body = braceBody(src, m.index)
    if (body) ranges.push(body)
  }
  return ranges
}

const RUST_FN = /^(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*\(/gm
const TS_FNS = [
  /(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*(?:<[^>]*>)?\s*\(/g,
  /(?:export\s+)?const\s+([A-Za-z_$][\w$]*)\s*(?::[^=]{0,120})?=\s*(?:async\s*)?\([^)]*\)\s*(?::[^=]{0,120})?=>\s*\{/g,
]

function duplicateCandidates(src, kind, minLen) {
  const out = []
  const pats = kind === 'rust' ? [RUST_FN] : TS_FNS
  const skip = kind === 'rust' ? cfgTestRanges(src) : []
  for (const re of pats) {
    re.lastIndex = 0
    let m
    while ((m = re.exec(src)) !== null) {
      const body = braceBody(src, m.index)
      if (!body) continue
      if (skip.some(([s, e]) => m.index >= s && m.index < e)) continue
      const normalized = src.slice(body[0], body[1]).replace(/\s+/g, ' ').trim()
      if (normalized.length < minLen) continue
      out.push({ name: m[1], normalized, line: lineOf(src, m.index) })
    }
  }
  return out
}

/** Group candidates by name+body; return the clusters with ≥2 members. */
function duplicateClusters(entries) {
  const byKey = new Map()
  for (const e of entries) {
    const key = e.name + '\u0000' + e.normalized
    if (!byKey.has(key)) byKey.set(key, [])
    byKey.get(key).push(e)
  }
  const clusters = []
  for (const members of byKey.values()) {
    if (members.length < 2) continue
    clusters.push({
      name: members[0].name,
      paths: [...new Set(members.map((m) => m.path))].sort(),
      members,
    })
  }
  return clusters
}

// ── Run ──────────────────────────────────────────────────────────────────────

const failures = []
const notes = []

const files = SOURCE_DIRS.flatMap((d) => walk(join(ROOT, d)))
const sources = new Map() // repo-relative path -> comment-stripped text
for (const f of files) sources.set(relative(ROOT, f).split(sep).join('/'), stripComments(readFileSync(f, 'utf8')))

/** The Rust tree, kept as a SECOND map rather than folded into `sources`
 *  because every TS rule above is written against the TS spelling of its
 *  subject and would go looking in the api crate for `fetch(`. Only the
 *  language-agnostic rules (duplicate bodies) read both. */
const RUST_DIRS = ['api/crates']
const RUST_EXTS = ['.rs']
const rustSources = new Map() // repo-relative path -> comment-stripped text
for (const f of RUST_DIRS.flatMap((d) => walk(join(ROOT, d), [], RUST_EXTS))) {
  rustSources.set(relative(ROOT, f).split(sep).join('/'), stripComments(readFileSync(f, 'utf8')))
}

// Rules: forbidden outside `allow`.
for (const rule of RULES) {
  const found = []
  // A rule whose subject lives in the api crate reads `rustSources`; every
  // other rule reads the TS tree. Without this the Rust rules below would scan
  // the SPA for `tracing::error!` and report "all clean" over nothing.
  const scanned = rule.lang === 'rust' ? rustSources : sources
  for (const [path, src] of scanned) {
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
  // A named contract step (the roll-safety guard below) is a deliberate one.
  const MARKER = /--\s*(deliberate|contract):\s*\S/
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
          line: lineOf(stripped, start + (statements[index].index ?? 0)),
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

// ROLL-SAFETY GUARD: the expand-contract rule, enforced (docs/UPDATES.md →
// "The overlap contract"). The previous release keeps serving on a schema this
// array migrated: through a roll's drain, after an aborted roll (green goes,
// its migrations stay), and through the 24 h Roll back window. A statement
// that takes away something the previous release still reads or writes is an
// outage on rollback even when no datum is lost, and the destructive guard
// above cannot see that. The git-hosts block (#611) passed it while it deleted
// a setting with a WHERE, dropped the defaults the old inserts leaned on, and
// swapped primary keys under the old ON CONFLICT targets; on rollback the old
// release saw GitHub as disconnected and its workbench inserts failed NOT NULL
// (upgrade-2, release sweep 2026-11-02).
//
// A contract step belongs to the release AFTER the one whose code stopped
// reading the thing, and it says which one: an inline
// `-- contract: <what goes>, unread since <vX.Y.Z or #PR>`. `-- deliberate:`
// does not pass these. It records that losing data is intended, not that the
// previous release is done with the thing.
//
// The baseline is the first statement after the #611 compensation. Everything
// before it is frozen by the checksum law, and #611's own steps were
// compensated by appending, not by editing them. The number is a literal and
// never auto-updates.
{
  const PG = 'ui/src/server/db/pg.ts'
  const CONTRACT_GUARD_FROM_INDEX = 526
  const MARKER = /--\s*contract:[^\n]*(#\d+|\bv\d+\.\d+)/i
  const PATTERNS = [
    [/\bdrop\s+(table|column)\b/, 'drop table/column: the previous release reads it'],
    [/\brename\s+(to|column)\b/, 'rename: the previous release queries the old name'],
    [/\balter\s+column\s+[^;]*\b(type|using)\b/, 'alter column type: the previous release binds the old type'],
    [/\balter\s+column\s+\S+\s+drop\s+default\b/, 'drop default: the previous release inserts without the column'],
    [/\balter\s+column\s+\S+\s+set\s+not\s+null\b/, 'set not null: the previous release inserts without the column'],
    [
      /\bdrop\s+constraint\s+(if\s+exists\s+)?\S+_(pkey|key)\b/,
      'drop a primary key or unique constraint: the previous release names it in ON CONFLICT',
    ],
    [/\bdelete\s+from\s+app_settings\b/, 'delete an app setting: the previous release reads it by key'],
  ]
  // Same normalization as the destructive guard: quoted literals blanked (a
  // value cannot fire a pattern), -- comments stripped, lowercased. A
  // dollar-quoted function body is code that runs later, not a step this pass
  // takes, so it is blanked too.
  const contractStep = (sql) => {
    const norm = sql
      .replace(/\$\$[\s\S]*?\$\$/g, '$$$$')
      .replace(/'(?:[^'\\]|'')*'/g, "''")
      .replace(/--[^\n]*/g, '')
      .toLowerCase()
    return PATTERNS.find(([test]) => test.test(norm))?.[1] ?? null
  }
  const stripped = stripComments(readFileSync(join(ROOT, PG), 'utf8'))
  const start = stripped.indexOf('const MIGRATIONS: string[] = [')
  const body = stripped.slice(start, stripped.indexOf('\n]', start))
  const statements = [...body.matchAll(/`([^`]*)`/g)]
  // The guard proves itself on the statements it exists for: #611's contract
  // steps, verbatim, must still be in the array and must still be caught. A
  // pattern edit that would have let them through fails here.
  const SHIPPED = [
    "delete from app_settings where key = 'github_config'",
    "alter table workbench_repos alter column host_id drop default",
    "alter table workbench_repo_flow alter column host_id drop default",
    "alter table workbench_repos drop constraint if exists workbench_repos_pkey",
    "alter table workbench_repo_flow drop constraint if exists workbench_repo_flow_pkey",
  ]
  const missed = SHIPPED.filter((s) => !statements.some(([, sql]) => sql.trim() === s) || !contractStep(s))
  if (missed.length) {
    failures.push({
      id: 'pg-migration-contract-guard-blind',
      what: "the roll-safety guard no longer catches #611's contract steps (or they left the MIGRATIONS array)",
      fix: [
        'These statements broke a rollback to the previous release. The guard must keep',
        'catching their shapes; fix PATTERNS in scripts/check-invariants.mjs, never the list.',
      ],
      found: missed.map((s) => ({ path: PG, line: 0, text: s })),
    })
  }
  if (statements.length >= CONTRACT_GUARD_FROM_INDEX) {
    const offenders = []
    statements.forEach(([text, sql], index) => {
      if (index < CONTRACT_GUARD_FROM_INDEX || MARKER.test(text)) return
      const step = contractStep(sql)
      if (step)
        offenders.push({
          line: lineOf(stripped, start + (statements[index].index ?? 0)),
          text: `${step}: ${text.trim().split('\n')[0].slice(0, 80)}`,
        })
    })
    if (offenders.length) {
      failures.push({
        id: 'pg-migration-contract-unmarked',
        what: `a MIGRATIONS statement (index >= ${CONTRACT_GUARD_FROM_INDEX}) takes away something the previous release may still read, and names no release that stopped reading it`,
        fix: [
          'Expand now, contract later (docs/UPDATES.md → The overlap contract). Ship the add with',
          'the code, and the drop in a later release. Mark it there with',
          '`-- contract: <what goes>, unread since <vX.Y.Z or #PR>` inside the statement.',
          'A `-- deliberate:` marker does not pass this check.',
        ],
        found: offenders.map((o) => ({ path: PG, line: o.line, text: o.text })),
      })
    }
  } else if (statements.length > 0) {
    failures.push({
      id: 'pg-migration-contract-baseline-lost',
      what: `the MIGRATIONS array parses as ${statements.length} statements, below the roll-safety guard's baseline of ${CONTRACT_GUARD_FROM_INDEX}`,
      fix: ['Statements are append-only — find what removed entries, or fix the parser in scripts/check-invariants.mjs.'],
      found: [],
    })
  }
}

// A TYPE-ONLY PACKAGE (@types/*) IS COMPILE-TIME, SO IT LIVES IN
// devDependencies — never dependencies, optionalDependencies or
// peerDependencies, in ANY tracked package.json.
//
// THE HISTORY THIS KEEPS FROM REPEATING: @types/nodemailer sat in
// ui/package.json's dependencies and rode every production install, shipping
// to every deploy while being usable at no point in it — type stubs are
// consumed by the compiler and erased, so a production install can never run
// them. It was pulled out by hand (GH #264). This rule is the tripwire: the
// next @types/* package placed the same way fails the tree instead of waiting
// for someone to notice the bloat.
//
// This is a standalone block rather than a RULES/CENSUS entry because both of
// those scan TS/Svelte source under SOURCE_DIRS — they cannot see a
// package.json. It reads the manifests directly, and discovers them through
// git (`git ls-files`), which keeps gitignored client subrepos under apps/ out
// of scope automatically; the static list of the repo's own manifests is the
// fallback for a tree where git is unavailable.
{
  const RUNTIME_SECTIONS = ['dependencies', 'optionalDependencies', 'peerDependencies']
  let manifests = []
  try {
    manifests = execFileSync('git', ['ls-files', '*package.json'], { cwd: ROOT, encoding: 'utf8' })
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean)
  } catch {
    // git failed or this is not a repository — the static fallback below applies
  }
  if (manifests.length === 0) {
    manifests = ['package.json', 'ui/package.json', 'mcp/package.json', 'cli/package.json', 'desktop/package.json']
  }
  const hits = []
  for (const rel of manifests) {
    let pkg
    try {
      pkg = JSON.parse(readFileSync(join(ROOT, rel), 'utf8'))
    } catch {
      continue // an unparseable manifest fails every other gate; not this rule's subject
    }
    for (const section of RUNTIME_SECTIONS) {
      for (const name of Object.keys(pkg[section] ?? {})) {
        if (name.startsWith('@types/')) hits.push({ path: rel, line: 0, text: `${section}: ${name}` })
      }
    }
  }
  if (hits.length) {
    failures.push({
      id: 'types-package-in-runtime-dependencies',
      what: 'a @types/* package sits in a runtime dependency section',
      fix: [
        'Move it to the devDependencies of that package.json. Type stubs are compile-time only:',
        'in dependencies (or optionalDependencies / peerDependencies) they ride production',
        'installs and ship to every deploy, where nothing can ever run them. That is exactly',
        'how @types/nodemailer got into ui/package.json dependencies and rode production',
        'installs until GH #264 pulled it out by hand. Every production install skips',
        'devDependencies — that is where type stubs live.',
        '',
        'If you believe a specific @types/* package genuinely belongs in a runtime section,',
        'make that argument in the PR — do not widen this rule to make it pass.',
      ],
      found: hits,
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
    ['api/crates/talaria-fitness-talaria-tools/src/lib.rs', 'fn reads_the_real_registrations_at_all'],
    ['api/crates/talaria-fitness-talaria-tools/src/lib.rs', 'fn models_every_tool_the_toolkit_registers'],
    ['api/crates/talaria-fitness-toolbox/src/sandbox.rs', 'fn every_catalog_tool_is_backed_and_every_backend_is_in_the_catalog'],
    ['api/crates/talaria-harness-defs/src/registry.rs', 'fn every_backend_is_exercised_by_a_test_or_a_harness_surface'],
    ['api/crates/talaria-hermes-skills/src/lib.rs', 'fn every_catalogued_hermes_pack_is_classified'],
    ['api/crates/talaria-hermes-skills/src/lib.rs', 'fn every_replaced_pack_has_a_signpost_skill_occupying_the_name'],
    ['api/crates/talaria-hermes-skills/src/lib.rs', 'fn every_replaced_path_names_a_catalogued_skill_not_a_category'],
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


// THE TOOLKIT'S PROSE IS A GATE HERE, NOT ONLY A STARTUP WARNING. mcp/src/index.ts
// audits its own tool descriptions when it boots (`auditRefusalProse`): a
// description that names "closed" without archival spends an agent a turn on
// a 403, and text that teaches an agent to drop a scope argument to get past
// a refusal widens its own disclosure. That audit is stderr-only, relayed at
// error level with every other boot line, and nothing in CI read it — the
// file's own comment said the fatal version belonged here. This is it: the
// registrations are parsed out of the SOURCE (the fitness crate's reader,
// ported — the module boots an MCP server on import, so nothing imports it),
// every `${CONSTANT}` is resolved from the file's own constants so the rule
// sees the whole sentence, and the SAME regexes (read out of the file, not
// copied) run over every description and every field's `.describe(...)`.
//
// The count lives here too. Three docs stated three different tool counts
// (51, 62, 84) because a number in prose is updated by nobody; the rule is
// that no doc states one, and the README names every tool instead.
{
  const indexPath = join(ROOT, 'mcp/src/index.ts')
  const src = existsSync(indexPath) ? readFileSync(indexPath, 'utf8') : ''
  const chars = [...src]

  /** Read a JS string expression (`'a'`, `"a"`, a template, several joined by
   *  `+`) starting at char index `from`. Template interpolations are RESOLVED
   *  through `consts` (an unknown expression becomes `«expr»`), not treated as
   *  the end of the string: the prose rules are about whole sentences. */
  const readStringExpr = (from, consts) => {
    let i = from
    let out = ''
    for (;;) {
      while (i < chars.length && /\s/.test(chars[i])) i++
      const q = chars[i]
      if (q !== "'" && q !== '"' && q !== '`') break
      i++
      let done = false
      while (i < chars.length) {
        const ch = chars[i]
        if (ch === '\\') {
          const n = chars[i + 1]
          out += n === 'n' ? '\n' : n === 't' ? '\t' : n
          i += 2
          continue
        }
        if (ch === q) {
          i++
          done = true
          break
        }
        if (q === '`' && ch === '$' && chars[i + 1] === '{') {
          const close = chars.indexOf('}', i)
          if (close < 0) return null
          const expr = chars.slice(i + 2, close).join('').trim()
          out += consts.has(expr) ? consts.get(expr) : `«${expr}»`
          i = close + 1
          continue
        }
        out += ch
        i++
      }
      if (!done) return null
      let j = i
      while (j < chars.length && /\s/.test(chars[j])) j++
      if (chars[j] !== '+') return { text: out, end: i }
      i = j + 1
    }
    return out ? { text: out, end: i } : null
  }

  // The file's string constants, in source order, each resolved through the
  // ones before it (LIVE_ONLY interpolates OFF_THE_TABLE).
  const consts = new Map()
  for (const m of src.matchAll(/^const ([A-Z][A-Z0-9_]*) =\s*(?:\/\/[^\n]*\n\s*)*/gm)) {
    const at = [...src.slice(0, m.index + m[0].length)].length
    const read = readStringExpr(at, consts)
    if (read) consts.set(m[1], read.text)
  }

  // The scope-drop patterns, read out of the file so there is ONE list.
  const scopeRules = []
  const arrayStart = src.indexOf('const TEACHES_SCOPE_DROP')
  const arrayEnd = arrayStart < 0 ? -1 : src.indexOf('\n]', arrayStart)
  if (arrayStart >= 0 && arrayEnd > arrayStart) {
    for (const m of src.slice(arrayStart, arrayEnd).matchAll(/\[\s*\/((?:\\.|[^/\\])+)\/([a-z]*),\s*'([^']*)'/g)) {
      try {
        scopeRules.push([new RegExp(m[1], m[2]), m[3]])
      } catch {
        /* reported below as unreadable */
      }
    }
  }
  const keepTheScope = consts.get('KEEP_THE_SCOPE') ?? ''
  const refusalGap = (text) => {
    if (!/\bclosed\b/i.test(text)) return null
    const archival = /\barchived\b/i.test(text)
    const board = /archived board/i.test(text)
    if (archival && board) return null
    return !archival ? 'names "closed" and nothing about archival' : 'names archival but not an ARCHIVED BOARD'
  }
  const scopeGap = (text) => {
    const t = keepTheScope ? text.split(keepTheScope).join(' ') : text
    for (const [rx, why] of scopeRules) if (rx.test(t)) return why
    return null
  }

  // Every registration: its description, then every `.describe(...)` until
  // the next registration (the field descriptions — the advice this gate
  // exists for lived in a field's text the second time).
  const regs = [...src.matchAll(/^server\.registerTool\(\s*'([a-z_]+)',\s*\{\s*description:\s*/gm)]
  const refusal = []
  const scope = []
  let described = 0
  for (let r = 0; r < regs.length; r++) {
    const m = regs[r]
    const name = m[1]
    const at = [...src.slice(0, m.index + m[0].length)].length
    const line = lineOf(src, m.index)
    const desc = readStringExpr(at, consts)
    if (!desc) {
      refusal.push({ path: 'mcp/src/index.ts', line, text: `${name}: description is not a readable string expression` })
      continue
    }
    described++
    const texts = [['<tool description>', desc.text]]
    const blockEnd = r + 1 < regs.length ? regs[r + 1].index : src.length
    const block = src.slice(m.index, blockEnd)
    for (const d of block.matchAll(/\.describe\(/g)) {
      const fieldAt = [...src.slice(0, m.index + d.index + d[0].length)].length
      const field = readStringExpr(fieldAt, consts)
      if (field) texts.push([`field at line ${lineOf(src, m.index + d.index)}`, field.text])
    }
    const why = refusalGap(desc.text)
    if (why) refusal.push({ path: 'mcp/src/index.ts', line, text: `${name}: ${why}` })
    for (const [where, text] of texts) {
      const bad = scopeGap(text)
      if (bad) scope.push({ path: 'mcp/src/index.ts', line, text: `${name} (${where}): ${bad}` })
    }
  }
  // The prose that teaches people to write agent prompts and skills is held
  // to the same scope rule: the README taught the trick, in words none of the
  // patterns caught, a round after the tool text stopped.
  for (const rel of ['mcp/README.md', 'scripts/skills/talaria-toolkit/SKILL.md']) {
    const abs = join(ROOT, rel)
    if (!existsSync(abs)) continue
    const lines = readFileSync(abs, 'utf8').split('\n')
    for (let i = 0; i < lines.length; i++) {
      const bad = scopeGap(lines[i])
      if (bad) scope.push({ path: rel, line: i + 1, text: `${bad}: ${lines[i].trim().slice(0, 120)}` })
    }
  }

  if (!src || described < 30 || scopeRules.length < 3 || !consts.has('KEEP_THE_SCOPE') || !src.includes('auditRefusalProse')) {
    failures.push({
      id: 'toolkit-prose-audit-unreadable',
      what: 'the toolkit prose gate could not read mcp/src/index.ts the way it expects to',
      fix: [
        `Parsed ${described} registrations, ${scopeRules.length} scope-drop patterns, KEEP_THE_SCOPE ${consts.has('KEEP_THE_SCOPE') ? 'found' : 'MISSING'},`,
        `auditRefusalProse ${src.includes('auditRefusalProse') ? 'found' : 'MISSING'}. This check reads registrations as`,
        "`server.registerTool('name', { description: <string>, …` and the patterns out of the",
        'TEACHES_SCOPE_DROP array (`[/…/flags, \'why\']` per line). If the file changed shape, update the',
        'reader in scripts/check-invariants.mjs — do NOT delete the check: it is what fails when the',
        'prose audit quietly stops existing.',
      ],
      found: [],
    })
  }
  if (refusal.length) {
    failures.push({
      id: 'toolkit-prose-refusal-drift',
      what: "a tool description names a refusal without all three conditions (closed, archived, archived board)",
      fix: [
        `A ticket refuses agent writes when it is ${consts.get('OFF_THE_TABLE') ?? 'closed, archived, or on an archived board'};`,
        'a description naming only "closed" spends an agent a turn on a 403. Interpolate LIVE_ONLY /',
        'COMMENT_EXEMPTION / OFF_THE_TABLE in mcp/src/index.ts instead of writing the sentence again.',
      ],
      found: refusal,
    })
  }
  if (scope.length) {
    failures.push({
      id: 'toolkit-prose-teaches-scope-drop',
      what: 'tool or playbook text teaches an agent to drop a scope argument to get past a refusal',
      fix: [
        "A scope argument (report_gap's taskId, report_problem's) is what binds an agent's free text to",
        'the people who can see the work it quotes; dropped, that text goes to every admin in the',
        'workspace. Say KEEP_THE_SCOPE (mcp/src/index.ts) — interpolate it, do not write a version of',
        'it — and in prose, delete the sentence.',
      ],
      found: scope,
    })
  }

  // The count: no doc states it. Lines that name a number of tools in the
  // toolkit's context and do not match the registration count fail.
  const registered = regs.length
  const COUNT_DOCS = [
    'README.md',
    'AGENTS.md',
    'CONTRIBUTING.md',
    'DEVELOPERS.md',
    'mcp/README.md',
    'api/crates/talaria-fitness-talaria-tools/src/lib.rs',
    ...(existsSync(join(ROOT, 'docs'))
      ? readdirSync(join(ROOT, 'docs'))
          .filter((f) => f.endsWith('.md'))
          .map((f) => `docs/${f}`)
      : []),
  ]
  const counts = []
  for (const rel of COUNT_DOCS) {
    const abs = join(ROOT, rel)
    if (!existsSync(abs)) continue
    const lines = readFileSync(abs, 'utf8').split('\n')
    for (let i = 0; i < lines.length; i++) {
      const m = /\b(\d+)\s+(?:registered\s+)?tools\b/i.exec(lines[i])
      if (!m) continue
      if (!/toolkit|talaria-mcp|index\.ts|registered tools|over MCP/i.test(lines[i])) continue
      if (Number(m[1]) !== registered) counts.push({ path: rel, line: i + 1, text: `says ${m[1]} tools; mcp/src/index.ts registers ${registered}` })
    }
  }
  // The README names every tool instead of counting them — held complete
  // here, because a table that lists 33 of 84 is how the count drifted in
  // the first place.
  const readmePath = join(ROOT, 'mcp/README.md')
  const readme = existsSync(readmePath) ? readFileSync(readmePath, 'utf8') : ''
  const unlisted = regs.map((m) => m[1]).filter((name) => !readme.includes(`\`${name}\``))
  if (unlisted.length) {
    failures.push({
      id: 'toolkit-readme-missing-tools',
      what: 'mcp/README.md does not name every tool mcp/src/index.ts registers',
      fix: [
        'Add a row for each under the family it belongs to in mcp/README.md § Tools (one line,',
        'the name in backticks). The README is the one place the whole toolkit is listed by name;',
        'a count in prose is what this replaced.',
      ],
      found: unlisted.map((name) => ({ path: 'mcp/README.md', line: 0, text: `\`${name}\` is registered and not listed` })),
    })
  }
  if (counts.length) {
    failures.push({
      id: 'toolkit-tool-count-drift',
      what: 'a doc states a toolkit tool count the source does not have',
      fix: [
        'Do not write the number down: it changes with every registration and nobody updates prose.',
        'Say "the toolkit\'s registered tools" and point at mcp/README.md, which names every tool',
        `(this check counts \`server.registerTool(\` in mcp/src/index.ts — currently ${registered}).`,
      ],
      found: counts,
    })
  }
}

// HERMES BUNDLED SKILLS MUST STAY CLASSIFIED, AND EVERY SKILL NAME IN A
// REPLACED PACK MUST BE OCCUPIED BY A TALARIA SIGNPOST. Hermes adds packs on
// image updates, and its skills_sync defers a bundled skill only when a skill
// in skills.external_dirs (/opt/skills) carries the same name — so a name
// with no signpost is a skill Hermes writes onto every agent. The catalog is
// scripts/hermes-skill-authority.json — classify a new path (replaced /
// keepExact / keepPrefix) before it teaches the fleet a parallel vault.
// Signposts live in scripts/skills/<signpost>/.
{
  const catalogPath = join(ROOT, 'scripts/hermes-skill-authority.json')
  if (!existsSync(catalogPath)) {
    failures.push({
      id: 'hermes-skill-authority-missing',
      what: 'scripts/hermes-skill-authority.json is missing',
      fix: [
        'The Hermes bundled-skill authority catalog is the tripwire for packs',
        'the image ships that conflict with Talaria. Restore the file, or the',
        'fleet will stop checking its signposts and teach Notion again.',
      ],
      found: [],
    })
  } else {
    const cat = loadAuthority(ROOT)
    const replaced = cat.replaced || []
    const unknown = unclassifiedHermes(cat.catalog || [], cat)
    if (unknown.length) {
      failures.push({
        id: 'hermes-skill-unclassified',
        what: 'Hermes bundled packs with no replace/keep classification',
        fix: [
          'Hermes shipped a pack this catalog has not classified. Add it to',
          'replaced (and a scripts/skills/<signpost> SKILL.md), keepExact, or',
          'keepPrefixes in scripts/hermes-skill-authority.json — do not let it',
          'teach the fleet a parallel workspace by omission.',
          ...unknown.map((u) => `  ${u}`),
        ],
        found: unknown.map((u) => ({ path: 'scripts/hermes-skill-authority.json', line: 1, text: u })),
      })
    }
    const missing = []
    for (const r of replaced) {
      const skill = join(ROOT, 'scripts/skills', r.signpost, 'SKILL.md')
      if (!existsSync(skill)) {
        missing.push(`${r.path} → scripts/skills/${r.signpost}/SKILL.md (${r.why})`)
      }
    }
    // A replaced path names one skill directory, never a category. Under a
    // category prefix, a sibling Hermes adds next week classifies as replaced
    // while no signpost holds its name, and nothing deletes it any more.
    const categories = replacedPathsNotCatalogued(cat)
    if (categories.length) {
      failures.push({
        id: 'hermes-skill-replaced-category',
        what: 'a replaced Hermes path is not a catalogued skill directory',
        fix: [
          'Name the skill (email/himalaya), not its category (email). A category',
          'prefix classifies every sibling Hermes adds as replaced while no',
          'signpost holds its name, so Hermes syncs it onto every agent.',
          ...categories.map((p) => `  ${p}`),
        ],
        found: categories.map((p) => ({ path: 'scripts/hermes-skill-authority.json', line: 1, text: p })),
      })
    }
    // Every name, not any one: the frontmatter `name:` of each signpost is
    // what Hermes matches, so a body that merely mentions a name is no cover.
    const declared = declaredSignpostNames(ROOT)
    for (const r of replaced) {
      for (const n of r.names || []) {
        if (!declared.has(n)) {
          missing.push(`${r.path} ships "${n}" → no scripts/skills/*/SKILL.md declares name: ${n}`)
        }
      }
    }
    if (missing.length) {
      failures.push({
        id: 'hermes-skill-signpost-missing',
        what: 'replaced Hermes packs without a Talaria signpost skill',
        fix: [
          'Hermes defers a bundled skill only to a same-named skill in',
          '/opt/skills, so an unoccupied name is a skill it syncs onto every',
          'agent. Add a short SKILL.md at scripts/skills/<signpost>/ whose',
          'frontmatter name: is the Hermes name, pointing at the',
          'talaria-toolkit playbook.',
          ...missing.map((m) => `  ${m}`),
        ],
        found: [],
      })
    }
  }
}

// A SUBREPO APP IS NOT PART OF THE UI'S TYPESCRIPT PROJECT.
//
// apps/ holds two kinds of app: in-repo apps (contacts) whose deps come from
// ui's node_modules, and client subrepos (gitignored, their own package.json
// and dependency tree — leadworks, waypoint). ui/tsconfig.json includes
// ../apps so in-repo apps typecheck under the UI project; a subrepo swept up
// by that include sprays a hundred phantom errors into every svelte-check —
// its imports resolve against ITS node_modules, which the UI check never
// installs ("Cannot find module 'zod'" from ../apps/leadworks, every run, on
// every machine with the subrepo checked out — which is why CI, without the
// subrepos, disagreed with every local run).
//
// The exclude list in ui/tsconfig.json is the fix; this check keeps it honest.
// A subrepo that is absent (fresh clone, CI) matches nothing — the exclude
// entry is harmless against a missing path, and the check only fires where
// the subrepo is actually present.
{
  const appsDir = join(ROOT, 'apps')
  if (statSync(appsDir).isDirectory()) {
    const tsconfig = JSON.parse(readFileSync(join(ROOT, 'ui/tsconfig.json'), 'utf8'))
    const excluded = new Set(tsconfig.exclude ?? [])
    const notExcluded = readdirSync(appsDir).filter(
      (name) =>
        statSync(join(appsDir, name)).isDirectory() &&
        existsSync(join(appsDir, name, 'package.json')) &&
        !excluded.has(`../apps/${name}`),
    )
    if (notExcluded.length) {
      failures.push({
        id: 'subrepo-app-inside-the-ui-tsconfig',
        what: 'a subrepo app (own package.json under apps/) is not excluded from ui/tsconfig.json',
        fix: [
          'Subrepo apps resolve imports against their own node_modules, which the UI typecheck',
          'never installs — including them sprays phantom "Cannot find module" errors into',
          'every svelte-check run. Add to ui/tsconfig.json:',
          ...notExcluded.map((name) => `  "exclude": [ ..., "../apps/${name}" ]`),
          '',
          'If the app is meant to be first-party (deps from ui), delete its package.json —',
          'that marker is what makes this check (and the exclude) treat it as a subrepo.',
        ],
        found: notExcluded.map((name) => ({
          path: `apps/${name}`,
          line: 0,
          text: 'subrepo app present but not in ui/tsconfig.json "exclude"',
        })),
      })
    }
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
    'api/crates/talaria-routes-knowledge/src/files/uploads_id.rs',
    'api/crates/talaria-routes-knowledge/src/files/artifacts_public_slug_download.rs',
  ]
  const RUST_SERVE = 'api/crates/talaria-uploads/src/lib.rs'

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

/** Clusters that are deliberately two answers to different questions, kept
 *  side by side. `paths` is the exact set of files (order-insensitive) and
 *  `why` states the reason; an entry whose paths stop forming a cluster fails
 *  as stale and asks to be deleted.
 *
 *  THIS LIST MUST SHRINK TO (NEARLY) EMPTY. It was seeded, not earned: every
 *  entry below is a real duplicate with a named wave that collapses it, and
 *  the wave deletes its entry in the same commit that lands the collapse. The
 *  stale check is what makes that mechanical — an entry left behind after the
 *  work is done fails the next `bun run check`, so the list cannot rot into a
 *  standing amnesty. Read the `why` as a to-do list, not as an exemption. */
const DUPLICATE_BODY_ALLOW = [
]

// Duplicate function bodies: one home per concept, across both languages.
{
  const tsEntries = []
  for (const [path, src] of sources) {
    if (path.endsWith('.test.ts')) continue
    for (const c of duplicateCandidates(src, 'ts', 100)) tsEntries.push({ ...c, path })
  }
  const rustEntries = []
  for (const [path, src] of rustSources) {
    for (const c of duplicateCandidates(src, 'rust', 120)) rustEntries.push({ ...c, path })
  }

  const tsClusters = duplicateClusters(tsEntries)
  const rustClusters = duplicateClusters(rustEntries)
  notes.push(
    `duplicate bodies: ${rustEntries.length} rust fns / ${rustClusters.length} clusters, ` +
      `${tsEntries.length} ts fns / ${tsClusters.length} clusters`,
  )

  const covered = new Set()
  // An entry names BOTH the function and its file set: the same two files can
  // hold two different duplicated functions (registry.rs and users.rs do), and
  // a path-only key would let one entry silently exempt the other's cluster —
  // an exemption wider than the argument that justified it. The name is part
  // of the claim, so an entry dies exactly when its own cluster does.
  const keyOf = (name, paths) => name + '\u0000' + paths.join('\u0000')
  const allowFor = (cluster) => {
    const hit = DUPLICATE_BODY_ALLOW.find(
      (a) =>
        a.name === cluster.name &&
        keyOf(a.name, [...a.paths].sort()) === keyOf(cluster.name, cluster.paths),
    )
    if (hit) covered.add(keyOf(hit.name, [...hit.paths].sort()))
    return hit
  }

  const report = (id, clusters) => {
    const found = []
    for (const c of clusters) {
      const allow = allowFor(c)
      if (allow) continue
      for (const m of c.members) found.push({ path: m.path, line: m.line, text: `fn ${m.name}(…)` })
    }
    if (!found.length) return
    failures.push({
      id,
      what: 'the same function body written out in more than one place',
      fix: [
        'Pick the crate or module that already owns the concept, make it `pub`, and import it',
        'everywhere else. A copy that is not deleted is a copy that drifts: the last sweep of',
        'this kind collapsed 47 `now_ms` copies and seven grew back within the week, because',
        'nothing failed when they did.',
        '',
        'If the two bodies coincide today but answer genuinely different questions, add a',
        'DUPLICATE_BODY_ALLOW entry in scripts/check-invariants.mjs naming the function, both',
        'paths and a `why` that states the difference — that is the argument to have in the PR.',
        'Do not widen the normalization to make a real duplicate pass.',
      ],
      found,
    })
  }
  report('duplicate-fn-body-rust', rustClusters)
  report('duplicate-fn-body-ts', tsClusters)

  for (const allow of DUPLICATE_BODY_ALLOW) {
    if (!covered.has(keyOf(allow.name, [...allow.paths].sort()))) {
      failures.push({
        id: 'duplicate-body-allow-stale',
        what: `a DUPLICATE_BODY_ALLOW entry no longer matches a duplicate cluster: ${allow.name} in ${allow.paths.join(', ')}`,
        fix: [
          'The named function is no longer the same body in every listed path — most likely',
          'because the wave that named it did its work. Delete the allow entry from',
          'scripts/check-invariants.mjs: an exemption held open for a cluster that no longer',
          'exists is held open for the next copy to walk through.',
        ],
        found: [],
      })
    }
  }
}

// A `OnceLock` EDGE THAT NOTHING SETS — the quietest bug shape in this tree.
//
// The api breaks dependency cycles with injected edges: a leaf crate declares
// `pub static FOO: OnceLock<Arc<dyn Fn…>>`, reads it with `FOO.get()`, and the
// wiring crate (api/crates/talaria-jobs) sets it at boot. The pattern is right.
// What it cannot survive is forgetting the set, because every layer of the
// compiler is happy: the declaration compiles, the read compiles, and the
// `None` arm is a VALID ANSWER — usually the permissive one. So the feature
// does not error, it simply never runs, and the fallback looks like a result.
//
// register_all's own comments record three of these found the hard way
// (ANNOUNCE_APPROVAL: every queued Google write waited five minutes for the
// sweep; BUILD_DISPATCH: every plan-draft POST answered 500; CONVERSATION_OWNER:
// the attribution ladder silently credited the wrong person). This rule exists
// because of the fourth: 82eb786e extracted the ticket-thread gate into its own
// crate behind a new `OnceLock` and never set it, so `ticket_message_relevant`
// answered `true` for every non-structural message for sixteen days — no gate
// at all, on every install. Its verification line was "cargo check -p
// talaria-api", which is exactly the tool that cannot see this.
//
// THE CENSUS BELOW IS A BACKLOG, NOT AN EXEMPTION LIST. Each entry is a read
// path that currently does nothing; what that costs differs per seam and none
// of them has been audited. The rule's job is to stop the set GROWING and to
// keep the known ones written down somewhere a person looks. Wire one and
// delete its line — the staleness check below forces that, so the census can
// only shrink.
const UNSET_SEAM_CENSUS = [
  // EMPTY, AND THAT IS THE POINT. It held twelve entries — every injected
  // edge a burst of crate extractions on 2026-09-18/19 declared and none of
  // them set. All twelve are now wired in `register_all` with a boot
  // assertion each, so the backlog is gone and the rule's only job from here
  // is to refuse the thirteenth.
]

{
  // `rustSources` is comment-stripped, so a seam merely NAMED in a comment
  // (this file's own history is full of them) cannot read as a setter.
  //
  // IT COVERS `api/crates` ONLY, and both halves of that matter here. A
  // setter in the BINARY crate would be invisible to it, so `api/src` is read
  // in as well — otherwise wiring an edge in main.rs would fail this rule on
  // correct code, and a rule that fires on correct code teaches people to
  // route around it. Test code is deliberately NOT read: a seam set only in
  // `api/tests/it/support` is wired for the suite and dead in production,
  // which is the exact bug this rule is for, so counting it would let the
  // rule certify the thing it exists to catch.
  const binarySrc = []
  for (const f of walk(join(ROOT, 'api/src'), [], ['.rs'])) {
    binarySrc.push(stripComments(readFileSync(f, 'utf8')))
  }
  const allRust = [...rustSources.values(), ...binarySrc].join('\n')
  const isSet = (name) => new RegExp(`\\b${name}\\s*\\.\\s*set\\s*\\(`, 's').test(allRust)
  const isRead = (name) => new RegExp(`\\b${name}\\s*\\.\\s*get\\s*\\(`, 's').test(allRust)

  const seams = []
  for (const [path, src] of rustSources) {
    const re = /pub static ([A-Z_0-9]+)\s*:\s*OnceLock</g
    let m
    while ((m = re.exec(src))) seams.push({ name: m[1], path, line: lineOf(src, m.index) })
  }
  notes.push(`oncelock seams: ${seams.length} declared, ${seams.filter((s) => !isSet(s.name)).length} unset`)

  const censused = new Set(UNSET_SEAM_CENSUS.map((c) => `${c.name}\u0000${c.path}`))
  const found = []
  for (const seam of seams) {
    if (isSet(seam.name)) continue
    // A seam nothing reads either is dead code, not a dead FEATURE — a
    // different (and milder) problem than a read path that silently no-ops.
    if (!isRead(seam.name)) continue
    if (censused.has(`${seam.name}\u0000${seam.path}`)) continue
    found.push({ path: seam.path, line: seam.line, text: `pub static ${seam.name}: OnceLock<…>` })
  }
  if (found.length) {
    failures.push({
      id: 'oncelock-seam-never-set',
      what: 'an injected edge is read but nothing anywhere sets it — the feature behind it cannot run',
      fix: [
        'Set it where every other edge is set: `register_all` in api/crates/talaria-jobs/src/lib.rs,',
        'which boot reaches through `jobs::arm`. One line per edge:',
        '',
        '    let _ = talaria_thing::MY_EDGE.set(Arc::new(|pg, id| {',
        '        Box::pin(async move { talaria_other::real_impl(&pg, &id).await })',
        '    }));',
        '',
        'WHY THIS IS A GATE AND NOT A LINT: `cargo check`, `cargo clippy` and every unit test',
        'over the declaring crate all pass with the edge unset, because the `None` arm is a',
        'valid answer — and it is usually the permissive one, so the feature reads as working.',
        'Four of these have shipped: a five-minute delay on every queued Google write, a 500 on',
        'every plan-draft POST, an attribution ladder crediting the wrong person, and a ticket',
        'gate that was simply absent for sixteen days.',
        '',
        'If the edge is genuinely optional — a hook only some deployments wire — add it to',
        'UNSET_SEAM_CENSUS in scripts/check-invariants.mjs and say so in the pull request. The',
        'census is a backlog that can only shrink, not a place to park a new one.',
      ],
      found,
    })
  }

// ── Every wired decision site is in the census ───────────────────────────────
//
// `talaria_decide::sites::DECIDE_SITES` is what the admin panel lists, what
// `gate_of` resolves a switch through, and what `acts_on` reads a floor from.
// A site that records to the shadow ledger under an id with no census entry is
// the worst of both: it costs a judgment per call and can never be switched on
// or tuned, because `gate_of` answers `{ on: false, floor: 1.0 }` for an id it
// does not know. It also reads as working — the call site falls back exactly as
// it did before, forever.
//
// This is the same class of failure as the nine cross-checks that were all
// failing unrun: somebody added a thing and did not register it. Cheap to
// catch, invisible otherwise.
{
  const sitesPath = 'api/crates/talaria-decide/src/sites.rs'
  const sitesSrc = rustSources.get(sitesPath)
  if (!sitesSrc) {
    failures.push({
      id: 'decide-site-census-missing',
      what: `${sitesPath} is gone — the decision-site census is what makes a wired site switchable`,
      fix: ['Restore it, or update this rule if the census moved.'],
      found: [],
    })
  } else {
    const census = new Set()
    for (const m of sitesSrc.matchAll(/^\s*id:\s*"([a-z0-9-]+)"\s*,/gm)) census.add(m[1])

    // Where every `const NAME: &str = "…"` lives, so a site named through one
    // resolves. Keyed by file, because `SITE` is declared in more than one
    // module with DIFFERENT values — a global name→value map would let an
    // uncensused `SITE` pass because some other module's `SITE` is censused,
    // which is the rule certifying the thing it exists to catch.
    const constsByFile = new Map()
    for (const [path, src] of rustSources) {
      const m2 = new Map()
      for (const m of src.matchAll(/const\s+([A-Z_0-9]+)\s*:\s*&(?:'static\s+)?str\s*=\s*"([a-z0-9-]+)"/g)) {
        m2.set(m[1], m[2])
      }
      constsByFile.set(path, m2)
    }
    // `a::b::NAME` resolves in the module the path names (a file called
    // `b.rs`), then in the referring file, then — only if the name is
    // unambiguous workspace-wide — anywhere.
    const resolve = (path, qualified) => {
      const parts = qualified.split('::')
      const name = parts[parts.length - 1]
      const modName = parts.length > 1 ? parts[parts.length - 2] : null
      if (modName) {
        for (const [p2, m2] of constsByFile) {
          if (p2.endsWith(`/${modName}.rs`) && m2.has(name)) return m2.get(name)
        }
      }
      const own = constsByFile.get(path)?.get(name)
      if (own) return own
      const seen = new Set()
      for (const m2 of constsByFile.values()) if (m2.has(name)) seen.add(m2.get(name))
      return seen.size === 1 ? [...seen][0] : null
    }

    const recorded = []
    for (const [path, src] of rustSources) {
      for (const m of src.matchAll(/\bsite:\s*(?:"([a-z0-9-]+)"|((?:[a-z_0-9]+::)*[A-Z_0-9]+))\s*,/g)) {
        const id = m[1] ?? resolve(path, m[2])
        // An identifier we cannot resolve is not evidence of a missing entry —
        // a rule that fires on correct code teaches people to route around it.
        if (!id) continue
        recorded.push({ id, path, line: lineOf(src, m.index) })
      }
    }
    notes.push(`decide sites: ${census.size} in the census, ${new Set(recorded.map((r) => r.id)).size} recording`)
    const found = recorded
      .filter((r) => !census.has(r.id) && r.path !== sitesPath)
      .map((r) => ({ path: r.path, line: r.line, text: `site: "${r.id}" — not in DECIDE_SITES` }))
    if (found.length) {
      failures.push({
        id: 'decide-site-not-in-census',
        what: 'a decision-port call site records under an id the site census does not declare — it can never be switched on or tuned',
        fix: [
          'Add an entry to DECIDE_SITES in api/crates/talaria-decide/src/sites.rs, with:',
          '',
          '  id            the same string the call site records under',
          '  acts          what changes when it acts, in one sentence an operator reads',
          '  default_on    false unless the site already acted before the census existed',
          '  reads         FLOOR_LEAN for a one-directional question (act on YES only),',
          '                FLOOR_CERTAINTY for a two-sided one, and for every choice/score',
          '  default_floor and floor_why — the number AND the cost asymmetry that chose it',
          '',
          'WHY THIS IS A GATE: `gate_of` answers { on: false, floor: 1.0 } for an id it does',
          'not know, so an uncensused site costs a judgment per call, records rows nothing',
          'reads, and can never be switched on. Nothing fails; the call site falls back',
          'exactly as it did before, forever.',
        ],
        found,
      })
    }
  }
}

  for (const c of UNSET_SEAM_CENSUS) {
    const seam = seams.find((s) => s.name === c.name && s.path === c.path)
    if (!seam) {
      failures.push({
        id: 'unset-seam-census-stale',
        what: `UNSET_SEAM_CENSUS names a seam that no longer exists: ${c.name} in ${c.path}`,
        fix: [
          'The declaration is gone — deleted, renamed, or moved. Delete the census line too: a',
          'backlog entry for something that is not there hides the next real one.',
        ],
        found: [],
      })
    } else if (isSet(c.name)) {
      failures.push({
        id: 'unset-seam-census-stale',
        what: `UNSET_SEAM_CENSUS still names ${c.name}, which is now wired`,
        fix: [
          'Somebody set this edge — the backlog got shorter. Delete its line from',
          'UNSET_SEAM_CENSUS in scripts/check-invariants.mjs so the census keeps meaning',
          '"these are still dark".',
        ],
        found: [],
      })
    }
  }
}

// INSTANCE IDENTITY, PINNED ACROSS THE LANGUAGE LINE.
//
// The app-database's container name is composed twice: the resident tier names
// it when it attaches to an app's Postgres (ui/src/server/app-db.ts), and the
// CLI names it again when it dumps every app DB for a backup
// (cli/src/cmd/backup.ts). They agree today by convention — "talaria-appdb-
// <instance>-<slug>", the instance being the worktree/devbox name with
// `talaria` as the fallback — and a drift between them would be discovered on
// the one afternoon somebody needs the dump, as a container that is not there.
//
// A pin, not an import: one end is TypeScript with `process.env` and the other
// is TypeScript with a passed-in env record, so the shared thing is the SHAPE
// (the prefix, the two variables, the fallback), which is what this reads.
{
  const ID_SOURCES = [
    ['ui/src/server/app-db.ts', 'process.env.TALARIA_WORKTREE'],
    ['cli/src/cmd/backup.ts', 'env.TALARIA_WORKTREE'],
  ]
  const problems = []
  for (const [path, envPrefix] of ID_SOURCES) {
    const src = sources.get(path) ?? readFileSync(join(ROOT, path), 'utf8')
    if (!/TALARIA_DEVBOX/.test(src)) problems.push(`${path}: no TALARIA_DEVBOX in the instance id`)
    if (!/TALARIA_WORKTREE/.test(src)) problems.push(`${path}: no TALARIA_WORKTREE in the instance id`)
    if (!envPrefix.split('.').every((part) => src.includes(part))) {
      problems.push(`${path}: expected the instance id to read ${envPrefix}`)
    }
    if (!/'talaria'/.test(src)) problems.push(`${path}: expected 'talaria' as the instance id fallback`)
    if (!/talaria-appdb-\$\{instanceId\(/.test(src)) {
      problems.push(`${path}: expected the container name to be \`talaria-appdb-\${instanceId(…)}-<slug>\``)
    }
  }
  if (problems.length) {
    failures.push({
      id: 'app-db-container-name-drift',
      what: 'the app-DB container name is composed differently on the two sides that spell it',
      fix: [
        'Both ends must compose `talaria-appdb-<instance>-<slug>`, where <instance> is',
        'TALARIA_WORKTREE || TALARIA_DEVBOX || "talaria". If one end moved or was renamed,',
        'fix the code rather than this pin — the backup looks the container up by this name.',
        ...problems.map((p) => `  ${p}`),
      ],
      found: [],
    })
  }
}

// THE BRANCH MODEL IS MACHINERY, AND MACHINERY GETS AN ANCHOR.
//
// docs/BRANCHES.md describes three long-lived branches — testing (the
// integration lane), rc (the release candidate) and main (the trunk, which
// takes only rc's verified promotion). On its own that page is worth nothing:
// what makes it true is .github/workflows/flow.yml (the server-side tripwire),
// scripts/flow-guard.mjs (the rules), scripts/hooks/pre-push (the local
// wiring), promote.yml (the promotion) and — outside this tree — the branch
// protection the doc tells a maintainer to apply.
//
// The failure this check exists for is the one this file has already paid for
// twice: a rule whose subject is renamed or deleted goes on passing while
// guarding nothing. Delete the guard, or let a second workflow take the name
// the promotion's evidence is found by, and every other check here stays green
// while the model becomes prose somebody wrote once. So each piece must exist,
// must still be wired to the others, and the evidence must keep one identity.
{
  const read = (rel) => (existsSync(join(ROOT, rel)) ? readFileSync(join(ROOT, rel), 'utf8') : null)
  const found = []

  const doc = read('docs/BRANCHES.md')
  if (doc === null) {
    found.push({ path: 'docs/BRANCHES.md', line: 0, text: 'the branch model has no home' })
  } else {
    for (const branch of ['rc', 'main']) {
      if (!new RegExp(`^\\|\\s*\`${branch}\``, 'm').test(doc)) {
        found.push({
          path: 'docs/BRANCHES.md',
          line: 0,
          text: `the model no longer lists \`${branch}\` as a long-lived branch`,
        })
      }
    }
  }

  // [file, what it must still contain, what that piece is for]
  const wired = [
    ['.github/workflows/flow.yml', 'pr-base', 'the guard that refuses a pull request to main from anywhere but rc'],
    ['.github/workflows/flow.yml', 'promotion-gate', 'the guard that requires a green rc-deploy run for the promoted commit'],
    ['.github/workflows/flow.yml', 'flow-guard.mjs', 'the policy script, wired server-side'],
    ['.github/workflows/flow.yml', '.head_branch == "rc"', "the promotion gate's evidence is rc's own runs, not any run of the sha"],
    ['.github/workflows/promote.yml', '.head_branch == "rc"', "the promotion's evidence is the gate's: rc's own runs"],
    ['scripts/hooks/pre-push', 'flow-guard.mjs', 'the same policy, wired before the push leaves the machine'],
    ['scripts/flow-guard.mjs', "'main'", 'the trunk the policy polices'],
    ['scripts/flow-guard.mjs', "'rc'", 'the release candidate the policy polices'],
    ['scripts/flow-guard.mjs', "'testing'", 'the integration lane the policy polices'],
  ]
  for (const [file, needle, why] of wired) {
    const text = read(file)
    if (text === null) {
      found.push({ path: file, line: 0, text: 'missing' })
    } else if (!text.includes(needle)) {
      found.push({ path: file, line: 0, text: `no longer mentions \`${needle}\` — ${why}` })
    }
  }

  // The evidence keeps one identity. flow.yml's promotion gate and promote.yml
  // find rc's evidence by workflow NAME (`CI`, `rc-deploy`) among the runs of
  // one commit, so a second workflow carrying either name would become that
  // evidence: green on a sha it never checked the way the gate means. Each
  // name belongs to one file, and each of those files keeps its name.
  const EVIDENCE = { CI: 'ci.yml', 'rc-deploy': 'rc-deploy.yml' }
  const workflows = join(ROOT, '.github', 'workflows')
  const nameOf = (text) => {
    const m = /^name:\s*(.+?)\s*$/m.exec(text)
    return m ? m[1].replace(/^(['"])(.*)\1$/, '$2') : null
  }
  const named = new Map()
  for (const file of readdirSync(workflows).sort()) {
    if (!file.endsWith('.yml') && !file.endsWith('.yaml')) continue
    const name = nameOf(readFileSync(join(workflows, file), 'utf8'))
    named.set(file, name)
    if (name !== null && Object.hasOwn(EVIDENCE, name) && EVIDENCE[name] !== file) {
      found.push({
        path: `.github/workflows/${file}`,
        line: 0,
        text: `named \`${name}\`: flow.yml's promotion gate and promote.yml find rc's evidence by workflow name; a second workflow named \`${name}\` would become that evidence`,
      })
    }
  }
  for (const [name, file] of Object.entries(EVIDENCE)) {
    if (!named.has(file)) {
      found.push({ path: `.github/workflows/${file}`, line: 0, text: `missing — the promotion's \`${name}\` evidence comes from it` })
    } else if (named.get(file) !== name) {
      found.push({
        path: `.github/workflows/${file}`,
        line: 0,
        text: `is named \`${named.get(file)}\`, not \`${name}\` — flow.yml's promotion gate and promote.yml look its runs up by that name`,
      })
    }
  }

  if (found.length) {
    failures.push({
      id: 'branch-flow-anchors',
      what: "the branch model's machinery and its home have drifted apart",
      fix: [
        'docs/BRANCHES.md is the model; .github/workflows/flow.yml, scripts/flow-guard.mjs and',
        'scripts/hooks/pre-push are what enforce it. If one of them moved or was renamed, update',
        'this check and the doc in the same commit — an anchor that points at nothing passes while',
        'guarding nothing, and the model becomes a paragraph somebody wrote once.',
        '',
        'If a guard is genuinely gone: that is a decision about how changes reach main, not a',
        'refactor. Make it in docs/BRANCHES.md and here, together, so the next session reads the',
        'same story from both.',
        '',
        'If a workflow name is the finding: `CI` belongs to ci.yml and `rc-deploy` to',
        'rc-deploy.yml, and nothing else may carry either. The promotion gate and promote.yml',
        'count a green run of that NAME, on rc, as the evidence that rc\'s tip was checked and',
        'deployed; give the other workflow its own name. Renaming one of the two is a change to',
        'the evidence itself — flow.yml, promote.yml (its workflow_run list too) and this check',
        'move together.',
      ],
      found,
    })
  } else {
    notes.push(`branch flow anchored; evidence names unique: ${Object.entries(EVIDENCE).map(([n, f]) => `${n} ← ${f}`).join(', ')}`)
  }
}

// THE SKILLS INDEX IS A PROMISE TO EVERY HARNESS THAT HAS NO NATIVE DISCOVERY.
//
// AGENTS.md carries the table Pi, Oh My Pi, Codex and anything else without
// native skill discovery read to decide which procedure to open. A skill with no
// row is invisible to them; a row with no skill sends them to a file that does
// not exist, which is how a procedure gets read by one harness and silently not
// by the rest. docs/AGENT-TOOLING.md has said this is enforced since it was
// written — this is the rule that makes the sentence true, in both directions,
// because whichever side drifts the other is still authoritative-looking.
{
  const skillsDir = join(ROOT, '.claude', 'skills')
  const agents = existsSync(join(ROOT, 'AGENTS.md')) ? readFileSync(join(ROOT, 'AGENTS.md'), 'utf8') : null

  if (!existsSync(skillsDir) || agents === null) {
    failures.push({
      id: 'skills-index-anchor-missing',
      what: `${agents === null ? 'AGENTS.md' : '.claude/skills/'} is gone, so the skills index guards nothing`,
      fix: [
        'The index in AGENTS.md and the procedures in .claude/skills/ are two halves of one thing:',
        'the file every harness loads, and the files it sends them to. If either moved, move this',
        'check with it — an anchor whose subject is renamed keeps passing on a tree that no longer',
        'contains it.',
      ],
      found: [],
    })
  } else {
    // TRACKED skills only. `.claude/skills/` is a working tree in a repository
    // several sessions share: another session's half-written skill is not this
    // invariant's business, and the index is a property of the committed tree.
    // If git cannot be asked, the skill is policed rather than exempted.
    const isTracked = (name) => {
      try {
        return (
          execFileSync('git', ['ls-files', '--', `.claude/skills/${name}`], {
            cwd: ROOT,
            encoding: 'utf8',
          }).trim() !== ''
        )
      } catch {
        return true
      }
    }
    const dirs = readdirSync(skillsDir, { withFileTypes: true })
      .filter((e) => e.isDirectory())
      .map((e) => e.name)
      .filter(isTracked)

    // Rows are checked against what is PRESENT, not what is tracked: a row is
    // written in the same change as the skill it points at, and failing on a
    // skill that merely is not committed yet would make `bun run check`
    // unusable in the middle of adding one.
    const present = readdirSync(skillsDir, { withFileTypes: true })
      .filter((e) => e.isDirectory())
      .map((e) => e.name)
    const found = []

    for (const name of dirs) {
      if (!existsSync(join(skillsDir, name, 'SKILL.md'))) {
        found.push({ path: `.claude/skills/${name}`, line: 0, text: 'a directory with no SKILL.md' })
      }
      if (!agents.includes(`.claude/skills/${name}/SKILL.md`)) {
        found.push({
          path: `.claude/skills/${name}/SKILL.md`,
          line: 0,
          text: 'no row in the AGENTS.md skills table',
        })
      }
    }

    for (const m of agents.matchAll(/\.claude\/skills\/([a-z0-9-]+)\/SKILL\.md/g)) {
      if (!present.includes(m[1])) {
        found.push({
          path: 'AGENTS.md',
          line: 0,
          text: `the index lists \`${m[1]}\`, which is not a skill directory`,
        })
      }
    }

    if (found.length) {
      failures.push({
        id: 'skills-index-drift',
        what: 'the AGENTS.md skills index and .claude/skills/ disagree',
        fix: [
          'Add the row (a skill no harness is pointed at is a procedure only its author knows), or',
          'delete the skill — and if the row is the stale side, remove it: an index entry that',
          'points at nothing costs every reader a failed read.',
          '',
          'New skills are `.claude/skills/<name>/SKILL.md` with frontmatter whose `name` matches the',
          'directory and whose `description` carries the trigger ("Use when…"). Repo tooling NEVER',
          'goes in scripts/skills/ — that tree is product surface, shipped into agent containers.',
        ],
        found,
      })
    }
  }
}

// THE POST-PR WATCHER IS THE GATE THE STOP GATE CANNOT BE.
//
// AGENTS.md says an agent may not report done while its open pull request has
// failing checks, pending checks, or merge conflicts against rc. The stop gate
// sees the local tree. What makes the sentence true is scripts/hooks/pr-watch.mjs
// (the exit) and the ship-a-change skill (the procedure that runs it and fixes).
// Delete either, or stop the script from blocking on a red check and a conflict,
// and the rule becomes a paragraph that passes every other check.
{
  const read = (rel) => (existsSync(join(ROOT, rel)) ? readFileSync(join(ROOT, rel), 'utf8') : null)
  const found = []
  const wired = [
    ['scripts/hooks/pr-watch.mjs', "'pr-watch: red'", 'a failing check blocks; it is not a pass'],
    ['scripts/hooks/pr-watch.mjs', "'pr-watch: conflict'", 'a merge conflict blocks; it is not a pass'],
    ['scripts/hooks/pr-watch.mjs', 'MERGEABLE', 'a pass requires GitHub to have computed mergeable, not an empty rollup'],
    ['AGENTS.md', 'scripts/hooks/pr-watch.mjs', 'the no-done rule points at the gate'],
    ['.claude/skills/ship-a-change/SKILL.md', 'scripts/hooks/pr-watch.mjs', 'the procedure runs the gate'],
  ]
  for (const [file, needle, why] of wired) {
    const text = read(file)
    if (text === null) {
      found.push({ path: file, line: 0, text: 'missing' })
    } else if (!text.includes(needle)) {
      found.push({ path: file, line: 0, text: `no longer mentions \`${needle}\` — ${why}` })
    }
  }
  if (found.length) {
    failures.push({
      id: 'pr-watch-anchors',
      what: 'the post-PR watcher and the no-done rule have drifted apart',
      fix: [
        'AGENTS.md forbids reporting dev work done while an open PR has failing checks, pending',
        'checks, or merge conflicts against rc. scripts/hooks/pr-watch.mjs is the gate (exit 2',
        'on red and on conflict; exit 0 only when checks are green and mergeable). The procedure',
        'is .claude/skills/ship-a-change/SKILL.md. If one of them moved, update this check in the',
        'same commit — an anchor that points at nothing passes while guarding nothing.',
      ],
      found,
    })
  }
}

// THE LOCAL COMPILE GATE IS SCOPED, AND CARGO IS CAPPED.
//
// AGENTS.md says the local pre-push gate is `bun run gate`, and that a workspace
// cargo is not it: that locks api/target and pins the machine. What makes the
// sentence true is scripts/gate.mjs (it never compiles the workspace), the two
// cargo configs (`jobs = 2`), and the CI lifts that keep a dedicated runner from
// inheriting the cap. Delete any of them and the next agent runs api:check on a
// laptop again.
{
  const read = (rel) => (existsSync(join(ROOT, rel)) ? readFileSync(join(ROOT, rel), 'utf8') : null)
  const found = []
  const wired = [
    ['AGENTS.md', 'bun run gate', 'the local pre-push command'],
    ['AGENTS.md', 'Do not run a workspace cargo', 'the prohibition agents keep re-learning the hard way'],
    ['.claude/skills/ship-a-change/SKILL.md', 'bun run gate', 'the procedure runs the scoped gate'],
    ['scripts/gate.mjs', 'not a workspace compile', 'the script states what it refuses to run'],
    ['api/.cargo/config.toml', 'jobs = 2', 'the local cpu and ram cap'],
    ['desktop/src-tauri/.cargo/config.toml', 'jobs = 2', 'the desktop workspace does not inherit the api cap'],
    ['.github/actions/setup-runtime/action.yml', 'CARGO_BUILD_JOBS', 'CI lifts the cap, or the api job inherits 2 and times out'],
    ['api/package.Dockerfile', 'ARG CARGO_BUILD_JOBS=2', 'a package build without the lift stays capped'],
    ['.github/workflows/api-package.yml', 'CARGO_BUILD_JOBS=$(nproc)', 'the package workflow is the lift'],
  ]
  for (const [file, needle, why] of wired) {
    const text = read(file)
    if (text === null) {
      found.push({ path: file, line: 0, text: 'missing' })
    } else if (!text.includes(needle)) {
      found.push({ path: file, line: 0, text: `no longer mentions \`${needle}\` — ${why}` })
    }
  }
  if (found.length) {
    failures.push({
      id: 'scoped-gate-anchors',
      what: 'the local compile gate and the cargo job cap have drifted apart',
      fix: [
        'AGENTS.md says the local pre-push gate is `bun run gate`, and that a workspace cargo',
        '(`api:check`, `desktop:check`, `cargo test` without `-p`) is not it — that locks',
        'api/target and pins the machine. scripts/gate.mjs is the gate. jobs = 2 in both cargo',
        'configs is the cap. setup-runtime and api-package.yml lift it for dedicated runners.',
        'If one of them moved, update this check in the same commit — an anchor that points at',
        'nothing passes while guarding nothing.',
      ],
      found,
    })
  }
}

// THE DESKTOP SHELL MAKES WEBVIEWS ONLY IN A MAIN-THREAD TURN OF ITS OWN.
//
// A sync `#[tauri::command]` runs inside the calling webview's IPC callback.
// On Windows that is a WebView2 handler, and a WebView2 created there never
// finishes: `activate_instance` did exactly that, and every first open of an
// instance froze Talaria Desktop on Windows for good. Linux and macOS create
// synchronously, so only a Windows run could see it — this sees it on any OS,
// before review. Callbacks are held to the same rule — window, menu, tray and
// page events, GTK signals, the notification-click entry points — because a
// window event fires synchronously inside a sync command's window operation on
// Windows. The judging (a call graph by name, hops, callbacks, what counts as a
// creation) and its fixture live in scripts/desktop-threading.mjs; the rule is
// stated at the top of desktop/src-tauri/src/commands.rs.
{
  const desktopSrc = 'desktop/src-tauri/src'
  const files = new Map()
  for (const f of walk(join(ROOT, desktopSrc), [], ['.rs'])) {
    files.set(relative(ROOT, f).split(sep).join('/'), readFileSync(f, 'utf8'))
  }
  const wrong = desktopThreadingSelfTest()
  const { violations, commands, callbacks, creators } = scanWebviewThreading(files)
  if (wrong.length || commands === 0 || creators.length === 0) {
    failures.push({
      id: 'desktop-webview-threading-blind',
      what: 'the desktop webview-threading tripwire can no longer see what it guards',
      fix: [
        'scripts/desktop-threading.mjs must flag its own fixture, and must find the shell\'s',
        `\`#[tauri::command]\`s and at least one webview creation under ${desktopSrc}. If the`,
        'commands or the creation moved (a new file, a renamed builder), point the scanner at',
        'them in the same commit — a tripwire that sees nothing passes while guarding nothing.',
      ],
      found: [
        ...wrong.map((text) => ({ path: 'scripts/desktop-threading.mjs', line: 0, text: `self-test: ${text}` })),
        ...(commands === 0 ? [{ path: desktopSrc, line: 0, text: 'no #[tauri::command] found' }] : []),
        ...(creators.length === 0 ? [{ path: desktopSrc, line: 0, text: 'no webview creation found' }] : []),
      ],
    })
  }
  if (violations.length) {
    failures.push({
      id: 'desktop-webview-created-inside-ipc',
      what: 'a desktop shell command can create a webview (or wait on WebView2) where Windows deadlocks',
      fix: [
        'A sync command runs inside the caller\'s WebView2 IPC callback: make it `async`, and run the',
        'creation as one main-thread turn with `on_main_turn(&app, move |app| …).await` (commands.rs).',
        'An async command must not create from its tokio worker either — the same hop keeps hide →',
        'create → show in one turn, the order WebKitGTK geometry needs. Code that may already be on',
        'the main thread — a callback: a window, menu, tray or page event, a GTK signal, a function',
        'marked `// webview-threading: callback` — queues with `queue_main_turn`. `run_on_main_thread`',
        'runs inline there, so it is not a way out. docs/DESKTOP.md, "Threading".',
      ],
      found: violations.map((v) => ({
        path: v.path,
        line: v.line,
        text: `${{ sync: 'sync command', worker: 'async command, from its tokio worker', callback: 'callback, inline' }[v.kind]}: ${v.chain.join(' → ')}`,
      })),
    })
  }
  notes.push(`desktop threading: ${commands} shell commands and ${callbacks} callbacks, none reaches webview creation inline`)

  // The drop guard every instance webview runs (DROP_GUARD in commands.rs):
  // a drag no zone took is "no drop here", never a navigation, and a drag
  // over a native field is left to the engine. Run against a stub DOM.
  const guardWrong = checkDropGuard(files.get(`${desktopSrc}/commands.rs`) ?? '')
  if (guardWrong.length) {
    failures.push({
      id: 'desktop-drop-guard',
      what: "the instance webviews' drop guard does not do what it promises",
      fix: [
        'DROP_GUARD (desktop/src-tauri/src/commands.rs) cancels a drag that no drop zone took —',
        'without it, the engine loads a dropped file or link in place of the instance — and must',
        'leave a drag over <input>, <textarea>, <select> or anything contenteditable to the engine,',
        'which takes those drops with nobody calling preventDefault. scripts/desktop-drop-guard.mjs',
        'runs the script against each case; if the guard now needs more of the DOM, grow its stub.',
      ],
      found: guardWrong.map((text) => ({ path: `${desktopSrc}/commands.rs`, line: 0, text })),
    })
  } else {
    notes.push('desktop drop guard: stray drags cancelled, zones and native fields left alone (stub DOM)')
  }

  // The reload keys every instance webview runs (RELOAD_KEYS in reload.rs):
  // Ctrl+R and F5 off macOS, Cmd+R on it, Shift to skip the cache, and every
  // other chord left to the page. Run against a stub window.
  const keysWrong = await checkReloadKeys(files.get(`${desktopSrc}/reload.rs`) ?? '')
  if (keysWrong.length) {
    failures.push({
      id: 'desktop-reload-keys',
      what: "the instance webviews' reload keys do not do what they promise",
      fix: [
        'RELOAD_KEYS (desktop/src-tauri/src/reload.rs) is how a person reloads an instance page,',
        'including one whose Talaria UI never loaded: Ctrl+R and F5 on Windows and Linux, Cmd+R on',
        'macOS, Shift (or Ctrl+F5) skipping the cache, and a refused desktop_reload still reloading.',
        'It must leave every other chord to the page. scripts/desktop-reload-keys.mjs presses each',
        'one; if the script now needs more of the window, grow its stub.',
      ],
      found: keysWrong.map((text) => ({ path: `${desktopSrc}/reload.rs`, line: 0, text })),
    })
  } else {
    notes.push('desktop reload keys: Ctrl+R/F5 (Cmd+R on macOS) reload, other chords left to the page (stub window)')
  }

  // The OS-link click script (OS_LINKS in links.rs): the one way a link
  // reaches the OS, so it takes only a trusted click on a mailto:, tel: or
  // sms: link that the page left alone. Run against a stub window.
  const osWrong = checkOsLinks(files.get(`${desktopSrc}/links.rs`) ?? '')
  if (osWrong.length) {
    failures.push({
      id: 'desktop-os-links',
      what: "the instance webviews' OS-link script hands over more, or less, than a person's click",
      fix: [
        'OS_LINKS (desktop/src-tauri/src/links.rs) is the only path from a page to the OS: the',
        'navigation and new-window handlers refuse every OS scheme, because no engine tells them a',
        'click from a script or the page from an artifact frame. It must take a trusted, primary-',
        'button click on a mailto:, tel: or sms: link the page did not handle, and nothing else.',
        'scripts/desktop-os-links.mjs clicks each case; if the script needs more of the DOM, grow it.',
      ],
      found: osWrong.map((text) => ({ path: `${desktopSrc}/links.rs`, line: 0, text })),
    })
  } else {
    notes.push('desktop OS links: only a trusted click on mailto:/tel:/sms: goes to the OS (stub window)')
  }
}

// THE DEV STACK'S PORTS, PINNED TO THE FILES THAT SPELL THEM.
//
// `cli/src/ports.ts` is the cli's single source for the host-port defaults, but
// each value is also spelled where it is USED: the compose file's
// `${VAR:-default}`, the Rust api's `DEFAULT_PORT`, the Dockerfile's `ENV PORT`.
// No import reaches across those, so this is a pin — it reads the cli's table
// and each authority's own spelling and fails when one moves without the others.
// A drifted port is a stack that half-boots: the app publishes 5273 while its
// healthcheck probes 5272, and every health gate says "unhealthy" forever.
{
  const portsSrc = readFileSync(join(ROOT, 'cli/src/ports.ts'), 'utf8')
  const cliValue = (name) => new RegExp(`export const ${name} = '([^']+)'`).exec(portsSrc)?.[1]
  const compose = readFileSync(join(ROOT, 'docker/dev-compose.yml'), 'utf8')
  const composeDefault = (varName) => new RegExp(`\\$\\{${varName}:-([0-9]+)\\}`).exec(compose)?.[1]
  const configSrc = readFileSync(join(ROOT, 'api/crates/talaria-config/src/lib.rs'), 'utf8')
  const rustDefault = /pub const DEFAULT_PORT: u16 = (\d+)/.exec(configSrc)?.[1]
  const dockerfile = readFileSync(join(ROOT, 'Dockerfile'), 'utf8')
  const envPort = /^ENV PORT=(\d+)/m.exec(dockerfile)?.[1]

  const checks = [
    ['API_PORT', cliValue('API_PORT'), rustDefault, 'api/crates/talaria-config/src/lib.rs DEFAULT_PORT'],
    ['APP_PORT', cliValue('APP_PORT'), envPort, "the Dockerfile's ENV PORT"],
    ['PG_PORT', cliValue('PG_PORT'), composeDefault('TALARIA_PG_PORT'), 'docker/dev-compose.yml'],
    ['REDIS_PORT', cliValue('REDIS_PORT'), composeDefault('TALARIA_REDIS_PORT'), 'docker/dev-compose.yml'],
    ['STORAGE_PORT', cliValue('STORAGE_PORT'), composeDefault('TALARIA_STORAGE_PORT'), 'docker/dev-compose.yml'],
  ]
  const problems = checks
    .filter(([, a, b]) => a === undefined || b === undefined || a !== b)
    .map(([name, a, b, where]) => `  ${name}: cli/src/ports.ts says ${a ?? '(missing)'}, ${where} says ${b ?? '(not found)'}`)
  if (problems.length) {
    failures.push({
      id: 'dev-port-drift',
      what: 'a dev stack port disagrees between the cli and the file that uses it',
      fix: [
        'Change the value in BOTH places — `cli/src/ports.ts` is the cli\'s single source, and',
        'the file named above is the authority for what actually binds. If a port moved on',
        'purpose, move it here too and update this check only if a NEW file became the owner.',
        ...problems,
      ],
      found: [],
    })
  }
}

// AGENT CLEANUP IS A STEP OF EVERY TASK, AND THE STOP GATE IS THE BACKSTOP.
//
// AGENTS.md tells a finished task to remove the local workspace it created
// before the claim. What makes that true is the sentence (every harness loads
// it), the cleanup skill, and the stop gate running scripts/cleanup-sweep.mjs
// --gate. The gate is wired for every harness whose Stop event can block a
// turn. Delete the sentence, the call, or a wired hook, and the disk fills
// again while every other check stays green.
{
  const read = (rel) => (existsSync(join(ROOT, rel)) ? readFileSync(join(ROOT, rel), 'utf8') : null)
  const found = []
  const wired = [
    ['scripts/hooks/stop-check.mjs', 'cleanup-sweep.mjs', 'the stop gate runs the sweep'],
    ['scripts/hooks/stop-check.mjs', '--gate', 'the stop gate uses the non-surprise mode'],
    ['scripts/cleanup-sweep.mjs', 'export const POLICY', 'the thresholds have one home'],
    ['.claude/skills/cleanup/SKILL.md', 'Use when', 'the procedure carries its trigger'],
    ['AGENTS.md', '.claude/skills/cleanup/SKILL.md', 'the invariant points at the skill'],
    ['AGENTS.md', 'local workspace is gone', 'cleanup is a before-done step of every task'],
    ['.claude/settings.json', 'stop-check.mjs', 'Claude Code runs the stop gate'],
    ['.codex/hooks.json', 'stop-check.mjs', 'Codex runs the stop gate'],
  ]
  for (const [file, needle, why] of wired) {
    const text = read(file)
    if (text === null) found.push({ path: file, line: 0, text: 'missing' })
    else if (!text.includes(needle)) found.push({ path: file, line: 0, text: `no longer mentions \`${needle}\` — ${why}` })
  }
  if (found.length) {
    failures.push({
      id: 'cleanup-sweep-anchors',
      what: 'the artifact sweep and the stop gate have drifted apart',
      fix: [
        'A finished dev task removes the local workspace it created before the claim.',
        'That sentence lives in AGENTS.md ("local workspace is gone"). The procedure is',
        '.claude/skills/cleanup/SKILL.md. scripts/hooks/stop-check.mjs runs',
        'scripts/cleanup-sweep.mjs --gate after bun run check. Claude wires it from',
        '.claude/settings.json; Codex wires it from .codex/hooks.json. If one of them',
        'moved, update this check in the same commit — an anchor that points at nothing',
        'passes while guarding nothing.',
      ],
      found,
    })
  }
}

// THE MANAGE-SECTION VIEW GRANTS, PINNED ACROSS THEIR TWO HOMES.
//
// GH #308 shipped exactly this way: /studio was added to nav.ts's
// MANAGE_VIEWS (what Admin → People offers) but not to the Rust const of the
// same name (the api's authority) — a grant an admin could make that stopped
// working at the door. No import reaches across the two, so this is a pin:
// read each list's own spelling and fail when one moves without the other.
// (A third home, the TS server's MANAGE_VIEW_ROUTES mirror, is gone: the TS
// host asks the api for a caller's grants instead of resolving its own copy.)
{
  const navSrc = readFileSync(join(ROOT, 'ui/src/lib/nav.ts'), 'utf8')
  const navBlock = /export const MANAGE_VIEWS[^\n]*= \[([\s\S]*?)\n\]/.exec(navSrc)?.[1]
  const navRoutes = [...(navBlock ?? '').matchAll(/to: '([^']+)'/g)].map((m) => m[1])
  const rustSrc = readFileSync(join(ROOT, 'api/crates/talaria-users/src/lib.rs'), 'utf8')
  const rustBlock = /pub const MANAGE_VIEW_ROUTES: \[&str; \d+\] = \[([\s\S]*?)\];/.exec(rustSrc)?.[1]
  const rustRoutes = [...(rustBlock ?? '').matchAll(/"([^"]+)"/g)].map((m) => m[1])

  const homes = [
    ["ui/src/lib/nav.ts MANAGE_VIEWS (the UI's offer list)", navRoutes],
    ["api/crates/talaria-users/src/lib.rs MANAGE_VIEW_ROUTES (the api's authority)", rustRoutes],
  ]
  const canonical = navRoutes
  const problems = []
  if (!navRoutes.length) {
    problems.push('  ui/src/lib/nav.ts: the MANAGE_VIEWS array was not found — this check now guards nothing')
  }
  for (const [where, routes] of homes.slice(1)) {
    const missing = canonical.filter((r) => !routes.includes(r))
    const extra = routes.filter((r) => !canonical.includes(r))
    if (missing.length || extra.length || (!routes.length && canonical.length)) {
      problems.push(
        `  ${where}: says [${routes.join(', ') || '(not found)'}] — nav.ts says [${canonical.join(', ') || '(not found)'}]`,
      )
    }
  }
  if (problems.length) {
    failures.push({
      id: 'manage-view-route-drift',
      what: 'the manage-section view routes disagree between the UI list and the Rust api',
      fix: [
        'Add or remove the route in BOTH places — ui/src/lib/nav.ts MANAGE_VIEWS is the',
        "UI's offer list, and api/crates/talaria-users/src/lib.rs MANAGE_VIEW_ROUTES the api's",
        'authority (bump its [&str; N] to match). A view on one list but not the other is a',
        'grant that cannot be offered, or one that stops working at the door.',
        ...problems,
      ],
      found: [],
    })
  }
}

// mise.toml pins = CI's pins. The root mise.toml is how people and workbench
// agents get the toolchain; setup-runtime is how CI gets it. Two spellings of
// one pin drift the first time someone bumps only the file they were looking
// at, and a workbench then builds on a toolchain CI never tested. Node is
// compared by major, since CI pins `22.x` and mise `22`.
{
  const MISE = 'mise.toml'
  const ACTION = '.github/actions/setup-runtime/action.yml'
  const mise = readFileSync(join(ROOT, MISE), 'utf8')
  const action = readFileSync(join(ROOT, ACTION), 'utf8')
  const pinOf = (tool) => new RegExp(`^${tool}\\s*=\\s*"([^"]+)"`, 'm').exec(mise)?.[1]
  const defaultOf = (input) =>
    new RegExp(`^  ${input}:\\n(?:    .*\\n)*?    default: '([^']*)'`, 'm').exec(action)?.[1]
  const problems = []
  for (const [tool, norm] of [
    ['rust', (v) => v],
    ['bun', (v) => v],
    ['node', (v) => v?.split('.')[0]],
  ]) {
    const a = pinOf(tool)
    const b = defaultOf(tool)
    if (a === undefined || b === undefined) {
      problems.push(`${tool}: could not read the pin (${MISE}: ${a ?? 'missing'}, ${ACTION}: ${b ?? 'missing'})`)
    } else if (norm(a) !== norm(b)) {
      problems.push(`${tool}: ${MISE} says ${a}, ${ACTION} says ${b}`)
    }
  }
  if (problems.length) {
    failures.push({
      id: 'toolchain-pin-drift',
      what: `${MISE} and ${ACTION} pin different toolchain versions`,
      fix: [`Bump both files together; the comment at the top of ${ACTION} lists every pin site.`, ...problems],
      found: [],
    })
  }
}

// ── Boot-injected seams that nothing injects ─────────────────────────────────
// A `pub static X: OnceLock<Arc<dyn Fn…>>` is a SEAM: a crate that must not
// depend on another asks for a function to be handed in at boot, and
// `register_all` hands it in. When nothing does, `X.get()` is None for the
// life of the process and every caller quietly takes its fallback — no
// error, no log, no failing test, because a unit test supplies its own fake
// and the fake is always present.
//
// This has now shipped three times:
//   · CONVERSATION_OWNER  — the attribution ladder's chatter rung, "dead in
//                           production for as long as it did"
//   · KB_DOC_ALLOWS_READ  — every non-owner read of a kb-embedded upload
//                           answered 404; caught only by review
//   · TASK_BOARD_ID       — every run whose subject was a ticket was
//                           unwatchable by anyone: the agent pane, the Turns
//                           transcript and the run's event stream all 403
//
// KNOWN_DEAD is a census in the sense used above: the seams that are unset
// TODAY. A NEW one fails this check. Removing a name from the list because
// it is now set is always correct; adding one needs a reason in the PR.
{
  // Found by the audit that TASK_BOARD_ID prompted. Each is read and never
  // set, so each is a fallback taken forever. The consequence is spelled out
  // where it was traced and marked "needs triage" where it was not — an
  // honest list beats a confident wrong one, and every entry here is a bug
  // to fix rather than a decision to keep.
  const KNOWN_DEAD = new Map([
    ['ANNOUNCE_APPROVAL', 'a queued Google action announces nothing'],
    ['COMPUTE_ALERT_COUNT', "an admin's home always reads zero alerts"],
    ['GENERATE_TITLE', 'chats and plans are never auto-titled'],
    ['OWNS_AGENT', 'the "your personal assistant" skill grant never applies'],
    ['SYNC_PRIVATE_DOCS', 'a personal agent never syncs private docs'],
    ['WORKBENCH_TOOLS', 'the registry caches an EMPTY Workbench tool catalog'],
    ['SUMMARIZE_SKILL', 'needs triage'],
    ['ROOM_COMMENT_FANOUT', 'needs triage'],
    ['AUDIENCE', 'needs triage'],
    ['RESOLVE_KB_REF', 'needs triage — a kb ref may never resolve'],
    ['RESOLVE_ARTIFACT_REF', 'needs triage — an artifact ref may never resolve'],
    ['UPDATE_TASK', 'needs triage'],
    // Dispatch itself is wired through BUILD_DISPATCH, which IS set, so this
    // looks like a second seam left behind rather than a broken dispatch.
    ['MAYBE_DISPATCH_TICKET', 'needs triage — likely redundant with BUILD_DISPATCH'],
    ['TICKET_RELEVANT', 'needs triage'],
  ])

  const seamSources = new Map(rustSources)
  for (const rel of ['api/src/lib.rs', 'api/src/main.rs']) {
    const abs = join(ROOT, rel)
    if (existsSync(abs)) seamSources.set(rel, stripComments(readFileSync(abs, 'utf8')))
  }
  const all = [...seamSources.values()].join('\n')

  const DECL = /pub static ([A-Z][A-Z0-9_]*)\s*:\s*OnceLock<([\s\S]*?)>\s*=\s*OnceLock::new\(\)/g
  const dead = []
  for (const [rel, text] of seamSources) {
    for (const m of text.matchAll(DECL)) {
      const [, name, ty] = m
      // Only function seams. A OnceLock holding a Regex or a cache is not a
      // thing anybody forgets to wire — it initialises itself on first use.
      if (!/dyn\s+Fn|OnceLock<\s*fn|^\s*fn\s*\(/.test(ty) && !/\bfn\s*\(/.test(ty)) continue
      if (!new RegExp(`\\b${name}\\.get\\b`).test(all)) continue // declared, never read
      if (new RegExp(`\\b${name}\\.set\\s*\\(`).test(all)) continue // wired somewhere
      if (KNOWN_DEAD.has(name)) continue
      const line = text.slice(0, m.index).split('\n').length
      dead.push({ path: rel, line, text: `pub static ${name}: OnceLock<…> — read, never set` })
    }
  }
  if (dead.length) {
    failures.push({
      id: 'unset-boot-seam',
      what: 'a boot-injected seam is read but nothing ever sets it, so its callers silently take the fallback',
      fix: [
        'Set it where the others are set — `register_all` in api/crates/talaria-jobs/src/lib.rs —',
        'and add the same closure to `wire_boot_seams` in api/tests/it/support so the test',
        'binary matches production. A test binary never runs register_all, which is how two of',
        'these went dead before.',
        'Better still: if the crate can reach the function directly, delete the seam. TASK_BOARD_ID',
        'looked like a dependency-cycle break and was not — the module already called its',
        'neighbours directly, and the indirection only bought a way to forget.',
      ],
      found: dead,
    })
  }
}

// ── The image builds the ui from a context .dockerignore trims ──────────────
// The Dockerfile's build stage runs `bun run build` in ui/ over `COPY . .` —
// the repo minus .dockerignore. A ui build input outside ui/ that the ignore
// file leaves out compiles everywhere else (dev, the gate, every PR job: none
// of them builds the image) and fails only in rc-deploy, after the merge.
// That shipped once: #604's SDK kit read cli/src/cmd/app/skeleton.ts and
// docs/sdk/talaria.schema.json, both ignored, and every rc deploy failed from
// that merge until the ignore file named them.
//
// The inputs are what the ui build can reach: relative imports out of ui/ in
// its non-test sources and in the scripts its `build` runs, plus the
// `join(UI|ui, '..', '<path>')` reads those scripts make.
{
  /** .dockerignore's own matching: patterns are relative to the context
   *  root, `*` stays within a segment, `**` crosses them, a pattern that
   *  matches a directory takes everything under it, and the LAST matching
   *  line wins, so a `!` line re-admits a file its directory's line left out. */
  const ignoreRules = existsSync(join(ROOT, '.dockerignore'))
    ? readFileSync(join(ROOT, '.dockerignore'), 'utf8')
        .split('\n')
        .map((l) => l.trim())
        .filter((l) => l && !l.startsWith('#'))
        .map((l) => {
          const keep = l.startsWith('!')
          const pat = (keep ? l.slice(1) : l).replace(/^\/+/, '').replace(/\/+$/, '')
          const re = pat
            .split(/(\*\*\/|\*\*|\*|\?)/)
            .map((t) => (t === '**/' ? '(?:.*/)?' : t === '**' ? '.*' : t === '*' ? '[^/]*' : t === '?' ? '[^/]' : t.replace(/[.+^${}()|[\]\\]/g, '\\$&')))
            .join('')
          return { keep, re: new RegExp(`^${re}$`) }
        })
    : []
  const ignored = (path) => {
    const parts = path.split('/')
    const prefixes = parts.map((_, i) => parts.slice(0, i + 1).join('/'))
    let out = false
    for (const r of ignoreRules) if (prefixes.some((p) => r.re.test(p))) out = !r.keep
    return out
  }

  const uiPkg = JSON.parse(readFileSync(join(ROOT, 'ui/package.json'), 'utf8'))
  const buildScripts = [...(uiPkg.scripts?.build ?? '').matchAll(/\bbun\s+(scripts\/[\w./-]+\.(?:ts|mjs|js))/g)].map((m) => `ui/${m[1]}`)
  const inputs = new Map()
  for (const [rel, text] of sources) {
    if (rel.startsWith('ui/src/') && !/\.test\.[a-z]+$/.test(rel)) inputs.set(rel, text)
  }
  for (const rel of buildScripts) {
    const abs = join(ROOT, rel)
    if (existsSync(abs)) inputs.set(rel, stripComments(readFileSync(abs, 'utf8')))
  }

  const IMPORT = /(?:\bfrom\s*|\bimport\s*\(\s*|\bimport\s+)['"](\.\.?\/[^'"]+)['"]/g
  const UP_READ = /\bjoin\(\s*(?:UI|ui|uiRoot\(\))\s*,\s*['"]\.\.['"]\s*,\s*['"]([^'"]+)['"]/g
  const norm = (p) => {
    const out = []
    for (const s of p.split('/')) {
      if (s === '..') out.pop()
      else if (s && s !== '.') out.push(s)
    }
    return out.join('/')
  }
  const left = []
  for (const [rel, text] of inputs) {
    const dir = rel.split('/').slice(0, -1).join('/')
    const hits = [
      ...[...text.matchAll(IMPORT)].map((m) => ({ index: m.index, target: norm(`${dir}/${m[1]}`) })),
      ...[...text.matchAll(UP_READ)].map((m) => ({ index: m.index, target: norm(m[1]) })),
    ]
    for (const { index, target } of hits) {
      if (target.startsWith('ui/')) continue
      // An extensionless import names a module; the file it resolves to is
      // what the context has to hold.
      const file = ['', '.ts', '.js', '.mjs', '/index.ts'].map((x) => target + x).find((p) => existsSync(join(ROOT, p))) ?? target
      if (!ignored(file)) continue
      left.push({ path: rel, line: lineOf(text, index), text: `reads ${file}, which .dockerignore leaves out of the image's build context` })
    }
  }
  if (left.length) {
    failures.push({
      id: 'image-build-input-ignored',
      what: 'the ui build reads a file outside ui/ that .dockerignore leaves out, so the image build fails in rc-deploy',
      fix: [
        'Re-admit exactly that file in .dockerignore with a `!` line under the line that',
        'leaves its directory out (see the "Not part of any build" block), with a note',
        'saying what reads it. Do not un-ignore the whole directory: the context stays a',
        'build of the source, not the machine.',
        'Or move the file under ui/ and import it from there.',
      ],
      found: left,
    })
  }
}

// ── The notification class registry is the one class list (C14) ──────────────
//
// The registry lives in api/crates/talaria-notify/src/classes.rs: KIND_CLASS
// maps every kind a writer files to its class, and CLASSES is the class list
// the wire serves as `classes[]` (labels, blurbs and defaults included). Two
// ways it rots, one half each:
//
//   RUST — a writer files `NotificationInput { kind: "<new-kind>" }` that the
//   table never heard of. It does not fail: `notify_class_of` drops it into
//   the quiet bucket, and the person's matrix has no row that controls it.
//   Every literal kind must be a KIND_CLASS kind or a class id.
//
//   TS — the client grows its own hand-kept class list (three or more
//   `id: '<class id>'` entries in one file). The server owns the copy now; a
//   second list is the first one drifting. notify-classes.ts is the one list
//   allowed until W2-C retires it with this allowlist line.
//
// The anchors fail when the registry parses empty — a rule whose subject was
// renamed keeps passing on a tree that no longer contains it.
{
  const registryPath = 'api/crates/talaria-notify/src/classes.rs'
  const registry = rustSources.get(registryPath) ?? ''
  const tableAt = registry.indexOf('pub const KIND_CLASS')
  const table = tableAt === -1 ? '' : registry.slice(tableAt, registry.indexOf('];', tableAt))
  const kinds = new Set([...table.matchAll(/\(\s*"([a-z0-9_-]+)"\s*,\s*"([a-z0-9_]+)"\s*,/g)].map((m) => m[1]))
  const classesAt = registry.indexOf('pub const CLASSES')
  const classesBody = classesAt === -1 ? '' : registry.slice(classesAt, registry.indexOf('];', classesAt))
  const classIds = new Set([...classesBody.matchAll(/\bid:\s*"([a-z0-9_]+)"/g)].map((m) => m[1]))
  if (kinds.size === 0 || classIds.size === 0) {
    failures.push({
      id: 'notify-class-registry-anchor',
      what: `${registryPath} no longer parses: KIND_CLASS gave ${kinds.size} kinds and CLASSES ${classIds.size} classes`,
      fix: [
        'The class registry moved or changed shape. Point this rule at its new home in the same',
        'commit, so the two checks below keep reading the real list — a rule over an empty list',
        'passes on everything.',
      ],
      found: [],
    })
  } else {
    // Rust half: every kind a writer files, in the two literal shapes that
    // carry one — `NotificationInput { kind, … }`, and the tickets' own
    // `TaskNotification { kind: "…".into(), … }` that becomes one — as a
    // string literal, or as a `const NAME: &str = "…"` it names (the MCP
    // health pass's `KIND_SERVER_DOWN`). A kind passed through a variable is
    // read where its caller spells it in one of these shapes.
    const kindConsts = new Map()
    for (const src of rustSources.values()) {
      for (const c of src.matchAll(/\bconst\s+([A-Z][A-Z0-9_]+)\s*:\s*&(?:'static\s+)?str\s*=\s*"([^"]+)"/g)) {
        if (!kindConsts.has(c[1])) kindConsts.set(c[1], c[2])
      }
    }
    const unknown = []
    let seen = 0
    for (const [path, src] of rustSources) {
      const skip = cfgTestRanges(src)
      for (const m of src.matchAll(/\b(?:NotificationInput|TaskNotification)\s*\{/g)) {
        if (skip.some(([s, e]) => m.index >= s && m.index < e)) continue
        const body = braceBody(src, m.index)
        if (!body) continue
        const k = /\bkind:\s*(?:"([^"]+)"|([A-Z][A-Z0-9_]+)\b)/.exec(src.slice(body[0], body[1]))
        if (!k) continue
        const kind = k[1] ?? kindConsts.get(k[2])
        if (kind === undefined) continue
        seen++
        if (!kinds.has(kind) && !classIds.has(kind)) {
          const spelled = k[1] ? `"${kind}"` : `${k[2]} (= "${kind}")`
          unknown.push({ path, line: lineOf(src, m.index), text: `kind: ${spelled} — not in KIND_CLASS and not a class id` })
        }
      }
    }
    // The shapes' own anchor: a tree where neither shape names a kind any
    // more has moved its writers somewhere this rule no longer reads.
    if (seen === 0) {
      failures.push({
        id: 'notify-kind-writers-anchor',
        what: 'no NotificationInput or TaskNotification literal names a kind — the writers moved out of reach of the class-registry rule',
        fix: [
          'Point the Rust half of this rule at the struct literals that now carry a notification',
          'kind, in the same commit that moved them.',
        ],
        found: [],
      })
    }
    if (unknown.length) {
      failures.push({
        id: 'notify-kind-not-in-registry',
        what: 'a writer files a notification kind the class registry does not name — it lands in the quiet bucket and no setting controls it',
        fix: [
          `Add the kind to KIND_CLASS in ${registryPath} — (kind, class, the reason a row`,
          'of it shows) — with the class a person would look for it under. If no class fits,',
          'that is a registry change: add the class to CLASSES with its category and defaults,',
          'and say so in the pull request. Or name an existing class id as the kind.',
        ],
        found: unknown,
      })
    }
    // TS half: no second hand-kept class list.
    const CLASS_LIST_ALLOWED = [] // empty since W2-C retired notify-classes.ts's list; keep it empty
    const lists = []
    const counts = new Map()
    for (const [path, src] of sources) {
      if (!path.startsWith('ui/src/') || /\.test\.[a-z]+$/.test(path)) continue
      const n = [...src.matchAll(/\bid:\s*['"]([a-z0-9_]+)['"]/g)].filter((m) => classIds.has(m[1])).length
      counts.set(path, n)
      if (n >= 3 && !CLASS_LIST_ALLOWED.includes(path)) {
        lists.push({ path, line: 1, text: `${n} entries name a notification class id — a hand-kept class list` })
      }
    }
    for (const path of CLASS_LIST_ALLOWED) {
      if ((counts.get(path) ?? 0) < 3) {
        lists.push({ path, line: 1, text: 'allowlisted as a class list, but it no longer is one — delete the allowlist line' })
      }
    }
    if (lists.length) {
      failures.push({
        id: 'notify-class-list-in-the-client',
        what: 'the client keeps its own list of notification classes — the server serves the registry as `classes[]`',
        fix: [
          'Render from `classes` in GET /api/notifications (`ClassInfo` in ui/src/lib/notify-wire.ts):',
          'labels, blurbs, categories and defaults all come from the registry. A second list is',
          `the first one drifting. The one allowed list is in CLASS_LIST_ALLOWED in this file`,
          'until W2-C retires it; when it goes, its allowlist line goes in the same commit.',
        ],
        found: lists,
      })
    }
    notes.push(`notification classes: ${classIds.size} in the registry, ${kinds.size} kinds in KIND_CLASS, ${seen} writer literals read`)
  }
}

// ── Every caller of ci.yml grants what ci.yml's jobs ask for ────────────────
// ci.yml's `what changed` job asks for `actions: read` (scripts/surfaces.mjs
// looks up the branch's last green run). A called workflow's job can request no
// more than the CALLING job grants, and a caller that grants less is not a red
// job — it is a startup failure, a run that dies before any job and that
// nobody is notified about. So every job that `uses:` ci.yml must say
// `actions: read` in its own `permissions:`, and ci.yml's `changes` job must
// still ask for it (or this check guards a grant nothing needs).
{
  const dir = join(ROOT, '.github', 'workflows')
  const found = []
  // The jobs of one workflow: [{id, line, body: [{n, text}]}], read by
  // indentation — two spaces for a job id, four for its keys.
  const jobsOf = (text) => {
    const lines = text.split('\n')
    const jobs = []
    let inJobs = false
    let job = null
    lines.forEach((raw, i) => {
      const line = raw.replace(/\s+#.*$/, '').replace(/^#.*$/, '')
      if (/^\S/.test(line)) {
        inJobs = /^jobs:\s*$/.test(line)
        job = null
        return
      }
      if (!inJobs) return
      const head = /^ {2}([A-Za-z0-9_-]+):\s*$/.exec(line)
      if (head) {
        job = { id: head[1], line: i + 1, body: [] }
        jobs.push(job)
      } else if (job) {
        job.body.push({ n: i + 1, text: line })
      }
    })
    return jobs
  }
  // Does the job's own `permissions:` grant actions read (or write, or all)?
  const grantsActions = (job) => {
    const at = job.body.findIndex((l) => /^ {4}permissions:/.test(l.text))
    if (at === -1) return false
    const value = job.body[at].text.replace(/^ {4}permissions:\s*/, '').trim()
    if (value) return /^(read-all|write-all)$/.test(value) || /\bactions:\s*(read|write)\b/.test(value)
    for (const l of job.body.slice(at + 1)) {
      if (l.text.trim() === '') continue
      if (!/^ {5,}/.test(l.text)) break
      if (/^\s+actions:\s*(read|write)\s*$/.test(l.text)) return true
    }
    return false
  }
  const callers = []
  for (const file of readdirSync(dir).sort()) {
    if (!file.endsWith('.yml') && !file.endsWith('.yaml')) continue
    const rel = `.github/workflows/${file}`
    const jobs = jobsOf(readFileSync(join(dir, file), 'utf8'))
    for (const job of jobs) {
      const calls = job.body.some((l) => /^ {4}uses:\s*['"]?\.\/\.github\/workflows\/ci\.yml['"]?\s*$/.test(l.text))
      if (!calls) continue
      callers.push(`${file}:${job.id}`)
      if (!grantsActions(job)) {
        found.push({ path: rel, line: job.line, text: `job \`${job.id}\` calls ci.yml without granting \`actions: read\`` })
      }
    }
    if (file === 'ci.yml') {
      const changes = jobs.find((j) => j.id === 'changes')
      if (!changes) {
        found.push({ path: rel, line: 0, text: 'no `changes` job — move this check with whatever plans the run now' })
      } else if (!grantsActions(changes)) {
        found.push({ path: rel, line: changes.line, text: '`changes` no longer asks for `actions: read` — the last-green-run lookup needs it' })
      }
    }
  }
  if (found.length) {
    failures.push({
      id: 'ci-call-grants',
      what: 'a workflow calls ci.yml without granting what its jobs request',
      fix: [
        'a nested job requesting more than its caller grants is a startup failure: the run dies',
        'before any job, as the Sep 2–16 nightlies did unnoticed. Give the calling job its own',
        '`permissions:` with `contents: read` and `actions: read` (release.yml\'s `ci` job and',
        'ci-offload.yml\'s `offload` job are the pattern). If ci.yml stops needing it, remove the',
        'grant from the `changes` job and this check in the same commit.',
      ],
      found,
    })
  } else {
    notes.push(`ci.yml callers grant actions: read: ${callers.join(', ') || 'none'}`)
  }
}

// ── No repository script in a write job the lanes can start ────────────────
// testing takes merges with one approval, or none by an admin; a ci/** push
// and a same-repository pull request run whatever their branch holds. GHCR's
// package write is not scoped by tag, so a job holding `packages: write` that
// runs a script from such a tree can re-point `:main` (the in-app updater's
// feed), `:latest` or `:nightly`; with `contents: write` it can push. So for
// every workflow whose `on:` takes a push to testing or ci/**, or a pull
// request, each job that holds `packages: write` or `contents: write` — its
// own grant, or the workflow's when it has none — may run no repository
// script: no `node scripts/`, `bun scripts/`, `bun run `, `./scripts/` or
// `bash scripts/` in a run line, and no local action (`uses: ./…`). A job that
// calls a local workflow with such a grant is followed into that workflow.
// testing-deploy.yml's split is the pattern: build and smoke under
// `packages: read`, publish from an artifact with no checkout.
//
// A job whose token is declared nowhere takes the repository's default
// workflow permissions (Settings → Actions → General, which should be read);
// those are named in the note, not failed. The rule proves itself on a
// fixture before it reads the tree.
{
  const dir = join(ROOT, '.github', 'workflows')
  const FORBIDDEN = /\bnode scripts\/|\bbun scripts\/|\bbun run |(?:^|[\s;&|(])\.\/scripts\/|\bbash scripts\//
  const strip = (raw) => raw.replace(/\s+#.*$/, '').replace(/^\s*#.*$/, '')
  const indent = (l) => /^ */.exec(l)[0].length

  // The lines of a top-level key's block, and its inline value.
  const topBlock = (lines, keyRe) => {
    const at = lines.findIndex((l) => keyRe.test(l))
    if (at === -1) return null
    const inline = lines[at].replace(keyRe, '').trim()
    const body = []
    for (let i = at + 1; i < lines.length; i++) {
      if (/^\S/.test(lines[i])) break
      body.push({ n: i + 1, text: lines[i] })
    }
    return { line: at + 1, inline, body }
  }
  // The keys at one indentation inside a block: name → {inline, body}.
  const keysAt = (body, depth) => {
    const out = new Map()
    let cur = null
    for (const l of body) {
      if (l.text.trim() === '') continue
      const m = new RegExp(`^ {${depth}}([A-Za-z0-9_-]+):\\s*(.*)$`).exec(l.text)
      if (m && indent(l.text) === depth) {
        cur = { line: l.n, inline: m[2].trim(), body: [] }
        out.set(m[1], cur)
      } else if (cur && indent(l.text) > depth) {
        cur.body.push(l)
      } else if (indent(l.text) <= depth) {
        cur = null
      }
    }
    return out
  }
  const writes = (perm) => {
    if (!perm) return null // declared nowhere
    const text = [perm.inline, ...perm.body.map((l) => l.text)].join(' ')
    return /\bwrite-all\b/.test(text) || /\b(contents|packages):\s*write\b/.test(text)
  }
  const globRe = (g) =>
    new RegExp(`^${g.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*\*/g, '\u0000').replace(/\*/g, '[^/]*').replace(/\?/g, '[^/]').replace(/\u0000/g, '.*')}$`)
  const listOf = (key) => {
    if (!key) return []
    const inline = /^\[(.*)\]$/.exec(key.inline)
    const items = inline ? inline[1].split(',') : key.body.map((l) => l.text.trim()).filter((t) => t.startsWith('- ')).map((t) => t.slice(2))
    return items.map((s) => s.trim().replace(/^(['"])(.*)\1$/, '$2')).filter(Boolean)
  }
  // Does `on:` take a push to testing or ci/**, or a pull request?
  const laneTriggered = (text) => {
    const lines = text.split('\n').map(strip)
    const on = topBlock(lines, /^(?:on|"on"|'on'):/)
    if (!on) return false
    if (on.inline) {
      const events = on.inline.replace(/[[\]{}]/g, ' ').split(/[\s,]+/)
      return events.some((e) => /^(pull_request|pull_request_target|push)$/.test(e))
    }
    const events = keysAt(on.body, 2)
    if (events.has('pull_request') || events.has('pull_request_target')) return true
    const push = events.get('push')
    if (!push) return false
    const flow = /branches:\s*\[([^\]]*)\]/.exec(push.inline)
    const sub = keysAt(push.body, 4)
    const branches = flow ? flow[1].split(',').map((s) => s.trim().replace(/^(['"])(.*)\1$/, '$2')) : listOf(sub.get('branches'))
    const filtered = flow || sub.has('branches') || sub.has('branches-ignore') || sub.has('tags') || sub.has('tags-ignore') || /\b(tags|branches-ignore)\b/.test(push.inline)
    if (!filtered) return true // every push
    if (sub.has('branches-ignore')) return true // the safe side: too many branches to rule out
    return branches.filter((b) => !b.startsWith('!')).some((b) => ['testing', 'ci/x', 'ci/full/x'].some((name) => globRe(b).test(name)))
  }
  const jobsIn = (text) => {
    const lines = text.split('\n').map(strip)
    const jobs = topBlock(lines, /^jobs:/)
    const perm = topBlock(lines, /^permissions:/)
    return { workflowPerm: perm, jobs: jobs ? keysAt(jobs.body, 2) : new Map() }
  }
  // The run lines (inline, or a block scalar's lines) and local uses of a job.
  const stepsOf = (job) => {
    const runs = []
    const local = []
    let block = -1
    for (const l of job.body) {
      if (block >= 0) {
        if (l.text.trim() === '' || indent(l.text) > block) {
          runs.push(l)
          continue
        }
        block = -1
      }
      const m = /^( *)(- )?(run|uses):\s*(.*)$/.exec(l.text)
      if (!m) continue
      const value = m[4].trim()
      if (m[3] === 'uses') {
        if (/^['"]?\.\//.test(value) && m[2]) local.push(l)
        continue
      }
      if (/^[|>][-+0-9]*$/.test(value)) block = m[1].length + (m[2] ? 2 : 0)
      else runs.push(l)
    }
    return { runs, local }
  }

  // files: Map<file name, text>. → {found, inspected, scope, undeclared}
  const scan = (files) => {
    const found = []
    const inspected = []
    const scope = []
    const undeclared = []
    const visit = (file, jobId, job, grant, via, depth) => {
      const rel = `.github/workflows/${file}`
      inspected.push(`${file}:${jobId}`)
      const { runs, local } = stepsOf(job)
      for (const l of runs) {
        if (FORBIDDEN.test(l.text)) {
          found.push({ path: rel, line: l.n, text: `job \`${jobId}\`${via} holds ${grant} and runs \`${l.text.trim()}\`` })
        }
      }
      for (const l of local) {
        found.push({ path: rel, line: l.n, text: `job \`${jobId}\`${via} holds ${grant} and runs a local action: \`${l.text.trim()}\`` })
      }
      const call = /^\.\/\.github\/workflows\/([^'"\s]+)/.exec(job.inline === '' ? (job.body.find((b) => /^ {4}uses:/.test(b.text))?.text.replace(/^ {4}uses:\s*['"]?/, '') ?? '') : '')
      if (!call || depth > 3) return
      const called = files.get(call[1])
      if (called === undefined) return
      const inner = jobsIn(called)
      for (const [id, j] of inner.jobs) {
        const own = keysAt(j.body, 4).get('permissions')
        const w = writes(own) ?? writes(inner.workflowPerm) ?? true // nothing declared: the caller's grant
        if (w) visit(call[1], id, j, grant, ` (called from ${file}:${jobId})`, depth + 1)
      }
    }
    for (const [file, text] of files) {
      if (!laneTriggered(text)) continue
      scope.push(file)
      const { workflowPerm, jobs } = jobsIn(text)
      for (const [id, job] of jobs) {
        const own = keysAt(job.body, 4).get('permissions')
        const w = writes(own) ?? writes(workflowPerm)
        if (w === null) {
          undeclared.push(`${file}:${id}`)
          continue
        }
        if (!w) continue
        const permText = [...(own ? [own.inline, ...own.body.map((l) => l.text)] : [workflowPerm.inline, ...workflowPerm.body.map((l) => l.text)])].join(' ')
        const grant = /write-all/.test(permText) ? '`write-all`' : [...permText.matchAll(/\b(contents|packages):\s*write\b/g)].map((m) => `\`${m[1]}: write\``).join(' and ')
        visit(file, id, job, grant, '', 0)
      }
    }
    return { found, inspected, scope, undeclared }
  }

  // The fixture: two violations the rule exists for, and the shape it allows.
  const FIXTURE = new Map([
    ['lane.yml', [
      'on:', '  push:', "    branches: ['testing']", 'permissions:', '  contents: write', 'jobs:',
      '  smoke:', '    runs-on: ubuntu-latest', '    steps:', '      - run: |', '          set -eu', '          node scripts/deploy-smoke.mjs --image x',
      '  pub:', '    uses: ./.github/workflows/pkg.yml', '    permissions:', '      packages: write',
      '  read:', '    permissions: {contents: read}', '    runs-on: ubuntu-latest', '    steps:', '      - run: bun run check',
    ].join('\n')],
    ['pkg.yml', [
      'on:', '  workflow_call: {}', 'jobs:', '  package:', '    permissions:', '      packages: write', '    steps:',
      '      - uses: ./.github/actions/setup-runtime', '      - run: docker buildx build .',
    ].join('\n')],
    ['offload.yml', ['on:', '  push:', "    branches: ['ci/**']", 'jobs:', '  prune:', '    permissions:', '      contents: write', '    steps:', '      - run: gh api x'].join('\n')],
    ['tags.yml', ['on:', '  push:', "    tags: ['v*']", 'permissions: write-all', 'jobs:', '  rel:', '    steps:', '      - run: bun run release'].join('\n')],
  ])
  const self = scan(FIXTURE)
  const want = ['lane.yml:12', 'pkg.yml:8']
  const got = self.found.map((f) => `${f.path.split('/').pop()}:${f.line}`).sort()
  const selfWrong = JSON.stringify(got) !== JSON.stringify(want) || !self.scope.includes('offload.yml') || self.scope.includes('tags.yml')
  const files = new Map()
  for (const file of readdirSync(dir).sort()) {
    if (file.endsWith('.yml') || file.endsWith('.yaml')) files.set(file, readFileSync(join(dir, file), 'utf8'))
  }
  const tree = scan(files)
  const found = [...tree.found]
  if (selfWrong) {
    found.push({ path: 'scripts/check-invariants.mjs', line: 0, text: `lane-write-jobs self-test: expected findings ${want.join(', ')} and offload.yml in scope, got ${got.join(', ') || 'none'} (scope: ${self.scope.join(', ')})` })
  }
  // The job this rule was written for has to be among those it reads.
  if (!tree.inspected.includes('testing-deploy.yml:publish')) {
    found.push({ path: '.github/workflows/testing-deploy.yml', line: 0, text: '`publish` was not inspected — the rule no longer sees the job it exists for; move the rule with the workflow' })
  }
  if (found.length) {
    failures.push({
      id: 'lane-write-jobs',
      what: 'code merged with one approval (or none, by an admin) must never run in a job that can write packages or contents — GHCR write is not scoped by tag',
      fix: [
        'Split the job: everything that runs the tree (a build, a smoke test, a script) goes in a job',
        'with `packages: read` and `contents: read` and a checkout with `persist-credentials: false`;',
        'it hands its result on as an artifact. The job that writes checks out nothing and runs only',
        'docker, gh and git against that artifact. testing-deploy.yml (build-smoke, then publish) is',
        'the pattern. If a write job only needs a value the tree computes, compute it in the read job',
        'and pass it as an output.',
      ],
      found,
    })
  } else {
    notes.push(
      `lane write jobs run no repository script: ${tree.inspected.join(', ') || 'none'} (in ${tree.scope.length} lane-triggered workflows)` +
        (tree.undeclared.length ? `; token undeclared, so the repository default (read): ${[...new Set(tree.undeclared.map((j) => j.split(':')[0]))].join(', ')}` : ''),
    )
  }
}

// ── GitHub's Latest release is the desktop updater feed ─────────────────────
// The shell's updater reads /releases/latest/download/latest.json, and GitHub
// marks a new non-prerelease release Latest unless told not to. A stable
// server release (release.yml) made Latest by default carries no latest.json at
// all (desktop installers ship only on desktop-v releases), and every installed
// app's update check 404s. So release.yml
// opens and edits every vX.Y.Z release --latest=false, and desktop-package.yml
// mints desktop-vX.Y.0 --latest=false and marks it Latest explicitly, only once
// its latest.json is uploaded. The reading of the workflows, and its fixture,
// live in scripts/release-latest.mjs.
{
  const dir = join(ROOT, '.github', 'workflows')
  const workflows = new Map()
  for (const file of readdirSync(dir)) {
    if (file.endsWith('.yml') || file.endsWith('.yaml')) workflows.set(file, readFileSync(join(dir, file), 'utf8'))
  }
  const confPath = join(ROOT, 'desktop/src-tauri/tauri.conf.json')
  const conf = existsSync(confPath) ? readFileSync(confPath, 'utf8') : null
  const wrong = releaseLatestSelfTest()
  const found = [
    ...wrong.map((text) => ({ path: 'scripts/release-latest.mjs', line: 0, text: `self-test: ${text}` })),
    ...checkReleaseLatest(workflows, conf),
  ]
  if (found.length) {
    failures.push({
      id: 'release-latest-is-the-desktop-feed',
      what: "a release can take GitHub's Latest flag from the desktop updater feed, or the feed can flip early",
      fix: [
        'GitHub\'s Latest release is what /releases/latest/download/latest.json resolves through, and',
        'that is the only URL the desktop updater reads (tauri.conf.json). release.yml creates and',
        'edits every vX.Y.Z release with --latest=false. desktop-package.yml creates its desktop-v',
        'mint with --latest=false and marks it Latest with `gh release edit "$TAG" --latest` after',
        'the latest.json upload, never before, and only inside `if [ "$MINT" = true ]` with MINT',
        'bound to needs.resolve.outputs.mint: `attach` also runs for a re-attach dispatch. Any',
        'other `gh release create` says --prerelease or an explicit --latest=false. RELEASING.md,',
        '"The desktop installers".',
      ],
      found,
    })
  } else {
    notes.push('release Latest flag: release.yml never takes it; desktop-package.yml takes it after latest.json lands, on the mint path only')
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
