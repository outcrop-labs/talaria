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
// PR C of the docs overhaul adds the second tripwire here: the SDK export coverage diff
// against docs/sdk/reference.md.

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
    for (const m of lines[i].matchAll(LINK_RE)) {
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

// ── Report (the check-invariants.mjs shape) ──────────────────────────────────

const BAR = '─'.repeat(78)
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
