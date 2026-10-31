// The judge's pull-request-body reading: what counts as a filled section.
// The reader this replaced returned "" for every section, so the judge asked
// "no Verified section filled in" on every pull request opened from the
// template — filled or not — and filling it in never cleared the ask. A wrong
// answer the other way waves an untouched template through as an answer.
// Run by `bun run check`.
import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { answer, section } from './pr-body.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const TEMPLATE = readFileSync(join(ROOT, '.github/PULL_REQUEST_TEMPLATE.md'), 'utf8')
let failed = 0
let ran = 0

function check(name, fn) {
  ran++
  try {
    fn()
  } catch (e) {
    failed++
    console.error(`FAIL ${name}: ${e instanceof Error ? e.stack : e}`)
  }
}

function eq(actual, expected) {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) throw new Error(`expected ${b}, got ${a}`)
}

function ok(cond, what) {
  if (!cond) throw new Error(what)
}

/** The template as an author leaves it after answering: the labels carry
 *  text, the changelog prompt is replaced by the entry. */
const FILLED = TEMPLATE.replace('- **Gate:**', '- **Gate:** `bun run gate` passed (check + the cli suite)')
  .replace('- **Exercised:**', '- **Exercised:** ran `talaria worktree probe` and read the seeded rows back')
  .replace(
    '<!-- Paste the entry from your changelog/ file, or say why the change is internal-only. -->',
    '- **The worktree waits for Postgres over TCP.** Verified: `bun run gate`, a real worktree.',
  )

check('the template still has the sections the judge reads', () => {
  ok(section(TEMPLATE, 'Verified') !== null, 'no ## Verified in the template')
  ok(section(TEMPLATE, 'CHANGELOG entry') !== null, 'no ## CHANGELOG entry in the template')
})

check('the template\'s checklist asks for a changelog/ file, never a hand edit of CHANGELOG.md', () => {
  // CHANGELOG.md is append-only and written by the roll (scripts/changelog-roll.mjs);
  // `bun run check` fails a hand-appended [Unreleased] bullet. A checklist that
  // names it sends the author to the one file they must not touch.
  const boxes = section(TEMPLATE, 'Checklist')
    .split('\n')
    .reduce((items, line) => {
      if (/^\s*- \[ \]/.test(line)) items.push(line)
      else if (items.length && /^\s+\S/.test(line)) items[items.length - 1] += ` ${line.trim()}`
      return items
    }, [])
  const entry = boxes.filter((b) => /changelog/i.test(b))
  eq(entry.length, 1)
  ok(/\[`changelog\/`\]\(\.\.\/changelog\/\)/.test(entry[0]), `the changelog item names no changelog/ file: ${entry[0]}`)
  ok(/changelog\/YYYY-MM-DD-<slug>\.md/.test(entry[0]), `the changelog item gives no file name: ${entry[0]}`)
  ok(!/CHANGELOG\.md\S* carries/.test(entry[0]), `the changelog item still asks for CHANGELOG.md: ${entry[0]}`)
  ok(/written only by the release roll/.test(entry[0]), entry[0])
})

check('a filled Verified section reads as its text, not "" (the old reader always said "")', () => {
  const v = answer(FILLED, 'Verified', TEMPLATE)
  eq(v, [
    '- **Gate:** `bun run gate` passed (check + the cli suite)',
    '- **Exercised:** ran `talaria worktree probe` and read the seeded rows back',
  ].join('\n'))
})

check('the untouched template answers nothing: labels, paragraphs, code block and comment are scaffold', () => {
  // The section itself is long — the template writes a lot under Verified —
  // which is exactly why "filled" cannot be a length test on the raw section.
  ok(section(TEMPLATE, 'Verified').length > 200, 'the raw Verified section is expected to be long')
  eq(answer(TEMPLATE, 'Verified', TEMPLATE), '')
  eq(answer(TEMPLATE, 'CHANGELOG entry', TEMPLATE), '')
})

check('one label answered is an answer; the other, still empty, is dropped', () => {
  const body = TEMPLATE.replace('- **Gate:**', '- **Gate:** bun run gate')
  eq(answer(body, 'Verified', TEMPLATE), '- **Gate:** bun run gate')
})

check('the last section in the body reads to the end of the input', () => {
  eq(answer(FILLED, 'CHANGELOG entry', TEMPLATE), '- **The worktree waits for Postgres over TCP.** Verified: `bun run gate`, a real worktree.')
  eq(section('## Verified\nran it, twice', 'Verified'), 'ran it, twice')
})

check('CRLF bodies (GitHub web edits) read the same as LF', () => {
  const crlf = FILLED.replace(/\n/g, '\r\n')
  eq(answer(crlf, 'Verified', TEMPLATE), answer(FILLED, 'Verified', TEMPLATE))
  eq(answer(crlf, 'CHANGELOG entry', TEMPLATE), answer(FILLED, 'CHANGELOG entry', TEMPLATE))
  eq(answer(TEMPLATE.replace(/\n/g, '\r\n'), 'Verified', TEMPLATE), '')
})

check('a section ends at the next level-2 heading, not at a level-3 one', () => {
  const body = '## Verified\n\n### Gate\n\nbun run gate\n\n## Branch flow\n\nno\n'
  eq(section(body, 'Verified'), '### Gate\n\nbun run gate')
  eq(section(body, 'Branch flow'), 'no')
})

check('a `## ` line inside a code fence does not cut the section', () => {
  const body = '## Verified\n\n```md\n## not a heading\n```\nexercised it\n\n## Next\n'
  eq(section(body, 'Verified'), '```md\n## not a heading\n```\nexercised it')
})

check('a fence closes on a bare run at least as long as its opener, never on an info string or a shorter run', () => {
  // A ```bash line inside an open fence is text. Closing on it would read `## inner`
  // as a section, then reopen a fence that swallows the real heading after it.
  const body = '## Verified\n```\n```bash\n## inner\n```\nx\n## CHANGELOG entry\ny\n'
  eq(section(body, 'Verified'), '```\n```bash\n## inner\n```\nx')
  eq(section(body, 'inner'), null)
  eq(section(body, 'CHANGELOG entry'), 'y')
  const longer = '## Verified\n````md\n```\n## not yet\n````\nran it\n## Next\nz\n'
  eq(section(longer, 'Verified'), '````md\n```\n## not yet\n````\nran it')
  eq(section(longer, 'Next'), 'z')
  // tildes and backticks do not close each other
  eq(section('## Verified\n~~~\n```\n## no\n~~~\n## Next\nz\n', 'Next'), 'z')
})

check('a heading inside an HTML comment does not render, so it does not count', () => {
  eq(section('<!--\n## Verified\nhidden\n-->\n## Other\n', 'Verified'), null)
})

check('heading names: case-insensitive, closing hashes, and `##Verified` is not a heading', () => {
  eq(section('## verified ##\nran it\n', 'Verified'), 'ran it')
  eq(section('##Verified\nran it\n', 'Verified'), null)
})

check('no such heading is null (a custom body is not an unfilled template)', () => {
  eq(answer('## Summary\n\nDid a thing.\n', 'Verified', TEMPLATE), null)
  eq(answer('', 'Verified', TEMPLATE), null)
})

check('a bullet that is only a bold sentence is an answer; `- **Gate**:` with nothing after is not', () => {
  eq(answer('## CHANGELOG entry\n\n- **Fixed the reader.**\n', 'CHANGELOG entry', TEMPLATE), '- **Fixed the reader.**')
  eq(answer('## Verified\n\n- **Gate**:\n- **Exercised:**  \n', 'Verified', TEMPLATE), '')
})

check('with no template on disk, the labels still do not count', () => {
  eq(answer('## Verified\n\n- **Gate:**\n- **Exercised:**\n', 'Verified', ''), '')
})

// ── the judge itself, end to end, on an empty diff ─────────────────────────
//
// `--base HEAD` makes the range empty, so the only findings are the body's.

function judge(body) {
  const dir = mkdtempSync(join(tmpdir(), 'judge-pr-body-'))
  try {
    const file = join(dir, 'body.md')
    writeFileSync(file, body)
    // Not the runner's own target: on the promotion pull request CI sets
    // GITHUB_BASE_REF=main, and a judge that inherited it would add
    // `targets-main` (a must, exit 2) to every body below.
    const env = { ...process.env }
    delete env.GITHUB_BASE_REF
    const run = spawnSync(process.execPath, ['scripts/judge-pr.mjs', '--base', 'HEAD', '--pr-body', file, '--json'], {
      cwd: ROOT,
      encoding: 'utf8',
      env,
    })
    if (run.status !== 0) throw new Error(`judge-pr exited ${run.status}: ${run.stderr}`)
    return JSON.parse(run.stdout).findings.map((f) => f.id).sort()
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

check('judge: the untouched template asks for Verified and for the entry', () => {
  eq(judge(TEMPLATE), ['pr-body-no-entry', 'pr-body-unfilled'])
})

check('judge: a filled body asks nothing about the body', () => {
  eq(judge(FILLED), [])
  eq(judge(FILLED.replace(/\n/g, '\r\n')), [])
})

check('judge: a body without the template headings is not judged as an unfilled template', () => {
  eq(judge('## Summary\n\nWhat changed.\n\n## Proof\n\nWhat ran.\n'), [])
})

check('judge: an empty body is named as empty', () => {
  eq(judge(''), ['empty-pr-body'])
})

if (failed) {
  console.error(`judge-pr: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`judge-pr: ${ran} checks, all clean`)
