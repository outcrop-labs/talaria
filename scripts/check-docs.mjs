#!/usr/bin/env node
// check-docs — the documentation tripwires.
//
// THE RULE. Every markdown link in the repo must resolve to a file that exists. A doc that
// points at nothing is worse than no doc: the reader cannot tell a moved file from a deleted
// feature. This check exists because the rot already happened once — `docs/m0-contract.md`
// linked `../adapter/README.md` for months after `adapter/` was deleted, and nothing failed.
//
// SCOPE. Every .md in the tree except:
//   - CHANGELOG.md — an append-only record; its links are frozen history, not promises.
//   - node_modules, .git, fleet/, logs/, worktrees/ — not documentation or not ours.
//   - apps/leadworks, playground, .inspect — gitignored client/demo trees.
//   - ui/.uploads — runtime artifact payloads that happen to be markdown.
//
// `.claude/` IS walked, and it did not used to be: the exemption's reasoning was
// that dotted directories are tooling rather than documentation, which is true
// of the config and false of `.claude/skills/` — every agent that follows a
// procedure reads those links, and an unenforced link surface is where a wrong
// path lives for months. The one thing skipped under it is `worktrees/`, which
// holds whole checkouts.
//
// WHAT IT DOES NOT CHECK. External http(s) links (the network is not CI's to interrogate) and
// same-document #anchors (heading ids are the renderer's business). A link that resolves to a
// file whose CONTENT is wrong is the reader's — and the docset's — problem, not this check.
//

import { readdirSync, readFileSync, statSync, existsSync } from 'node:fs'
import { join, dirname, resolve } from 'node:path'
import { diffReference, parseReference, sdkExports } from './sdk-reference.mjs'

const ROOT = resolve(import.meta.dirname, '..')

const EXEMPT_DIRS = new Set(['node_modules', 'fleet', 'logs', 'playground', 'worktrees'])
const EXEMPT_FILES = new Set(['CHANGELOG.md']) // append-only record; frozen links are history

/** Every markdown file under ROOT, minus the exemptions above. */
function walkMarkdown(dir, acc = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (entry.name.startsWith('.') && entry.name !== '.github' && entry.name !== '.claude') continue
      if (EXEMPT_DIRS.has(entry.name)) continue
      walkMarkdown(join(dir, entry.name), acc)
    } else if (entry.name.endsWith('.md')) {
      if (EXEMPT_FILES.has(entry.name)) continue
      acc.push(join(dir, entry.name))
    }
  }
  return acc
}

const files = walkMarkdown(ROOT)

const failures = []
let linksChecked = 0

// [text](target) — also the ![alt](src) image form, the <angle-bracket> target spelling, and
// an optional "title" suffix. Targets with a scheme (http, https, mailto, anything://) are the
// outside world's problem; a bare #fragment is a same-document anchor.
const LINK_RE = /\[([^\]]*)\]\(<([^>]+)>|(\[([^\]]*)\]\(([^)\s]+)(?:\s+"[^"]*")?\))/g

for (const file of files) {
  const rel = file.slice(ROOT.length + 1)
  const lines = readFileSync(file, 'utf8').split('\n')
  for (let i = 0; i < lines.length; i++) {
    // Inline code spans are CODE, not links: a generated schema cell like
    // `uuid[](50)` is markdown's `[text](target)` shape wearing backticks.
    // Mask the spans (length-preserving) before looking for links.
    const line = lines[i].replace(/(`+)[^`]*\1(?!`)/g, (mm) => ' '.repeat(mm.length))
    for (const m of line.matchAll(LINK_RE)) {
      const raw = m[2] ?? m[5]
      const text = m[1] ?? m[4] ?? ''
      if (!raw || /^[a-z][a-z0-9+.-]*:/i.test(raw)) continue // external scheme
      let target = raw.trim()
      if (target.startsWith('#')) continue // same-document anchor
      target = target.split('#')[0] // drop a file#anchor suffix
      if (!target) continue // was only ever an anchor
      linksChecked++
      // Entry files under changelog/ resolve against the REPO ROOT, their
      // post-roll home: the roll folds them into CHANGELOG.md at the root, so
      // a `./docs/…` link is wrong at rest but right the moment it ships.
      const base = file.startsWith(join(ROOT, 'changelog') + '/') ? ROOT : dirname(file)
      if (!existsSync(join(base, target))) {
        failures.push({
          path: rel,
          line: i + 1,
          text: `[${text}](${raw})`,
        })
      }
    }
  }
}


// ── Tripwire 2: SDK export coverage ─────────────────────────────────────────
// docs/sdk/reference.md claims to list EVERY export from both SDK entry points.
// This doc once listed exports that didn't exist (KeyHint) and missed ones that
// did (runHarness) for months. So the exports are parsed from the two source
// files and diffed both directions against the reference's rows — per entry
// point and per kind, because a row is a claim about where a symbol is
// imported from and what it is, not only that the name exists somewhere. The
// parser and the diff live in sdk-reference.mjs; check-docs.test.mjs pins them.

const SDK_ENTRIES = {
  '@talaria/sdk': 'ui/src/sdk/index.ts',
  '@talaria/sdk/server': 'ui/src/sdk/server.ts',
}
const SDK_REFERENCE = join(ROOT, 'docs/sdk/reference.md')

/** Source for an `@/…` specifier the SDK re-exports from, so a re-export's
 *  kind is the declaration's. Anything else (a package) stays unresolved. */
function resolveUiModule(spec) {
  if (!spec.startsWith('@/')) return null
  const base = join(ROOT, 'ui/src', spec.slice(2))
  for (const candidate of [`${base}.ts`, `${base}/index.ts`, base]) {
    if (existsSync(candidate) && statSync(candidate).isFile()) return readFileSync(candidate, 'utf8')
  }
  return null
}

const sdkExportsByEntry = new Map(
  Object.entries(SDK_ENTRIES).map(([entry, file]) => [entry, sdkExports(readFileSync(join(ROOT, file), 'utf8'), resolveUiModule)]),
)
const sdkDiff = existsSync(SDK_REFERENCE)
  ? diffReference(sdkExportsByEntry, parseReference(readFileSync(SDK_REFERENCE, 'utf8')))
  : { missing: ['docs/sdk/reference.md does not exist'], ghosts: [], kinds: [], dupes: [] }

// ── Report (the check-invariants.mjs shape) ──────────────────────────────────

const BAR = '─'.repeat(78)
const report = (id, what, hits, instead) => {
  console.error(`\n${BAR}\nFAIL  ${id}\n${BAR}`)
  console.error(`  ${what}:\n`)
  for (const h of hits) console.error(`    ${h}`)
  console.error('')
  console.error('  WHAT TO DO INSTEAD:')
  for (const l of instead) console.error('    ' + l)
}

if (sdkDiff.missing.length || sdkDiff.ghosts.length || sdkDiff.kinds.length || sdkDiff.dupes.length) {
  if (sdkDiff.missing.length)
    report('sdk-export-not-documented', 'exports with no row under their own entry point in docs/sdk/reference.md', sdkDiff.missing, [
      'Add a row under the entry point that exports it — the file claims completeness, and an',
      'app author imports from the entry point whose heading the row sits under.',
    ])
  if (sdkDiff.ghosts.length)
    report('sdk-doc-not-an-export', 'symbols docs/sdk/reference.md names that its entry point does not export', sdkDiff.ghosts, [
      'The docs name something the entry point does not export. Fix the row (or move it under',
      'the entry point that does export it) — this is how KeyHint survived for months.',
    ])
  if (sdkDiff.kinds.length)
    report('sdk-doc-wrong-kind', 'reference rows whose kind is not what the source exports', sdkDiff.kinds, [
      'Make the kind cell say what the export is: fn, const, class, comp or type. A type an',
      'author tries to call, or a component they try to call as a function, fails in their build.',
    ])
  if (sdkDiff.dupes.length)
    report('sdk-doc-duplicate-row', 'symbols with two rows under one entry point', sdkDiff.dupes, ['Keep one row.'])
  process.exitCode = 1
}

if (failures.length) {
  console.error(`\n${BAR}\nFAIL  doc-link-target-missing\n${BAR}`)
  console.error('  markdown links that point at files that do not exist:\n')
  for (const hit of failures) console.error(`    ${hit.path}:${hit.line}\n      ${hit.text}`)
  console.error('')
  console.error('  WHAT TO DO INSTEAD:')
  console.error('    If the target moved (git mv), update the link. If the target was deleted, the link')
  console.error('    should say so in prose or go away entirely — a dead link reads as a live promise.')
  console.error('    Historical docs may keep a deleted path as plain text, never as a link.')
  console.error(
    `\n${BAR}\n1 doc check failed. This is not lint — every rule here guards a rot that already\n` +
      'shipped once. If you believe a match is a false positive, say so in the PR.\n',
  )
  process.exit(1)
}

console.log(`docs: ${files.length} markdown files, ${linksChecked} links, all resolve`)
