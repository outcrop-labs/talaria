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
//   - node_modules, .git, .claude, fleet/, logs/ — not documentation or not ours.
//   - apps/leadworks, playground, .inspect — gitignored client/demo trees.
//   - ui/.uploads — runtime artifact payloads that happen to be markdown.
//
// WHAT IT DOES NOT CHECK. External http(s) links (the network is not CI's to interrogate) and
// same-document #anchors (heading ids are the renderer's business). A link that resolves to a
// file whose CONTENT is wrong is the reader's — and the docset's — problem, not this check's.
//

import { readdirSync, readFileSync, statSync, existsSync } from 'node:fs'
import { join, dirname, resolve } from 'node:path'

const ROOT = resolve(import.meta.dirname, '..')

const EXEMPT_DIRS = new Set(['node_modules', 'fleet', 'logs', 'playground'])
const EXEMPT_FILES = new Set(['CHANGELOG.md']) // append-only record; frozen links are history

/** Every markdown file under ROOT, minus the exemptions above. */
function walkMarkdown(dir, acc = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (entry.name.startsWith('.') && entry.name !== '.github') continue
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
      if (!existsSync(join(dirname(file), target))) {
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
// files and diffed both directions against the backticked identifiers in the
// reference — a missing export means "add a row", a documented-but-nonexistent
// one means "the docs lie".

/** Every name a module exports, from the declaration spellings used in
 *  ui/src/sdk/index.ts and server.ts: `export const/function/interface NAME`,
 *  `export type NAME =`, and `export { … }` clauses (with `as` aliases and
 *  per-item `type` prefixes, default-as-X re-exports). */
export function sdkExportNames(src) {
  const names = new Set()
  const id = '[A-Za-z_$][\\w$]*'
  for (const m of src.matchAll(new RegExp(`export\\s+(?:async\\s+)?(?:function|const)\\s+(${id})`, 'g'))) names.add(m[1])
  for (const m of src.matchAll(new RegExp(`export\\s+interface\\s+(${id})`, 'g'))) names.add(m[1])
  for (const m of src.matchAll(new RegExp(`export\\s+type\\s+(${id})\\s*=`, 'g'))) names.add(m[1])
  for (const m of src.matchAll(/export\s+(?:type\s+)?\{([^}]*)\}/g)) {
    for (const piece of m[1].split(',')) {
      const t = piece.trim().replace(/^type\s+/, '')
      if (!t) continue
      const as = new RegExp(`^${id}\\s+as\\s+(${id})$`).exec(t)
      if (as) names.add(as[1])
      else if (new RegExp(`^${id}$`).test(t)) names.add(t)
    }
  }
  return names
}

const SDK_CLIENT = readFileSync(join(ROOT, 'ui/src/sdk/index.ts'), 'utf8')
const SDK_SERVER = readFileSync(join(ROOT, 'ui/src/sdk/server.ts'), 'utf8')
const SDK_REFERENCE = join(ROOT, 'docs/sdk/reference.md')

const sdkExports = sdkExportNames(SDK_CLIENT + '\n' + SDK_SERVER)
const sdkDocNames = existsSync(SDK_REFERENCE) ? new Set([...readFileSync(SDK_REFERENCE, 'utf8').matchAll(/`([A-Za-z_$][\w$]*)`/g)].map((m) => m[1])) : new Set()

const sdkMissing = [...sdkExports].filter((n) => !sdkDocNames.has(n)).sort()
const sdkGhosts = [...sdkDocNames].filter((n) => !sdkExports.has(n)).sort()

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

if (sdkMissing.length || sdkGhosts.length) {
  if (sdkMissing.length)
    report('sdk-export-not-documented', 'exports with no row in docs/sdk/reference.md', sdkMissing, [
      'Add a row to docs/sdk/reference.md — the file claims completeness.',
    ])
  if (sdkGhosts.length)
    report('sdk-doc-not-an-export', 'backticked identifiers in docs/sdk/reference.md that are not exported', sdkGhosts, [
      'The docs name something neither SDK entry point exports. Fix the row (or the',
      'docs) — this is how KeyHint survived for months.',
    ])
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
