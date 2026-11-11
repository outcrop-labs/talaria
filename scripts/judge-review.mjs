#!/usr/bin/env node
// judge-review — a model reads every pull request: `judge (review)`.
//
// WHAT IT IS. The reading half of the pull-request judge. scripts/judge-pr.mjs
// holds the rules that are exact (branch flow, the changelog entry, the
// do-not-touch trees, the body's asks) and fails on a `must`. This script
// sends the change — its diff, the full files it touches, their callers and
// importers, the author's description and the repository's own rules — to a
// model on OpenRouter, keeps only the findings that survive code-side filters
// and a BLIND second reading, and reports at most three of them as line
// annotations. The calibration and the bars it must pass are in
// docs/AGENT-TOOLING.md → The judge.
//
// REPO TOOLING, NOT A FLEET AGENT. AGENTS.md's "Everything through the Talaria
// gateway" governs agent and persona model calls (CONTRIBUTING.md, "The rules
// that aren't style"). This script is CI tooling: it calls OpenRouter directly
// with a dedicated, spend-capped key named JUDGE_OPENROUTER_API_KEY. It never
// reads OPENROUTER_API_KEY — that is the app gateway's key
// (api/crates/talaria-gateway/src/provider.rs, ui/src/lib/models.ts), and a
// local `bun run judge` must never bill an operator's key.
//
// TRUST. In CI it runs from the TRUSTED branch's tree (.github/workflows/
// judge-review.yml checks out testing, rc or main — never the pull request's),
// and reads the pull request only as git objects: `git show`, `git diff --text
// --no-textconv --no-ext-diff`, `git grep -a`. Nothing from the pull request is
// executed, and .gitattributes comes from the trusted tree. A change to this
// script is therefore reviewed by the trusted branch's copy, not its own — a
// reviewer change carries a calibration scorecard instead (judge-pr asks).
//
// THE NETWORK. One host, OpenRouter, and only from callModel(). With
// JUDGE_REVIEW_OFFLINE=1 every call is refused unless it replays recorded
// streams (`--replay <dir>`, test-only and refused under GitHub Actions). The
// self-tests run offline, with no key in their environment.
//
// WHY NOT IN judge-pr.mjs. judge-pr is exact, fast and red on a `must`; this
// is a sampled reading that costs money and minutes and is never red. Mixing
// them would make the exact half wait on, and fail with, the sampled one.
//
// THE EXIT CONTRACT (scripts/hooks/README.md).
//   review --github   0 always (1 only on a usage error). A missing key, a
//                     fork, a provider error, a timeout, bad output, the cost
//                     ceiling and a crash are each a green job with a
//                     `not reviewed: <reason>` notice.
//   review (local)    0 reviewed, nothing to read; 2 reviewed, at least one
//                     confirmed finding to read; 1 could not run.
//   calibrate, canary, score, ledger   0 bar met / 2 bar missed / 1 could not
//                     run — under --github too. They never run on a pull request.
// NOTHING HERE MAY FAIL A PULL REQUEST. The only hold is agent-side, in
// scripts/hooks/pr-watch.mjs, off unless JUDGE_REVIEW_GATE=blockers.
//
// USAGE
//   node scripts/judge-review.mjs [review] [--base <ref|auto>] [--head <ref>] [--target <branch|auto>]
//       [--source <branch>] [--rules-from <ref>] [--pr <n>] [--pr-json <f>] [--comments-json <f>]
//       [--pr-body <f>] [--title <text>] [--prior <dir>] [--event <name>] [--action <name>] [--out <dir>]
//       [--github] [--json] [--dry-run] [--bootstrap] [--print-prompt <f>] [--no-verify] [--model <id>]
//       [--budget-tokens <n>] [--deadline <s>] [--record <dir>] [--replay <dir>]
//   node scripts/judge-review.mjs fetch-prior --repo <r> --pr <n|none> --head <sha> --run-id <id> --trusted <ref> --out <dir>
//   node scripts/judge-review.mjs calibrate [--set <f>] [--only <prs>|dev|heldout|clean] [--samples <1-3>]
//       [--concurrency 6] [--out <dir>] [--github] [--record <dir>] [--model <id>]
//   node scripts/judge-review.mjs score (--results <dir> | --ledger <ledger.json>) [--set <f>] [--grades <f>]
//   node scripts/judge-review.mjs ledger [--since 30d] [--out <dir>] [--escapes]
//   node scripts/judge-review.mjs canary [--github] [--health]
//
// The tests are scripts/judge-review.test.mjs (run by `bun run check`): offline.

import { execFileSync, spawnSync } from 'node:child_process'
import { createHash, randomBytes } from 'node:crypto'
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, posix, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { pickBase } from './surfaces.mjs'
import {
  BIDI_ZW_RE,
  escapeData,
  escapeProperty,
  findingId,
  findingTitle,
  norm,
  parseAnswers,
  readReview,
  safeTerminal,
  slug,
  VERDICT_TITLE,
  verdictMessage,
} from './judge-review-wire.mjs'

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const PROCESS_START = Date.now()

// ── constants ───────────────────────────────────────────────────────────────

/** The one literal of the model id among the workflows and scripts. */
export const DEFAULT_MODEL = 'z-ai/glm-5.3-flash'
/** USD per million tokens. `max_price` is sent only for a model listed here. */
export const PRICE_CAPS = { [DEFAULT_MODEL]: { prompt: 0.2, completion: 0.6 } }
export const ENDPOINT = 'https://openrouter.ai/api/v1/chat/completions'
export const KEY_ENV = 'JUDGE_OPENROUTER_API_KEY'
export const CALIBRATION_KEY_ENV = 'JUDGE_CALIBRATION_OPENROUTER_API_KEY'
const APP_KEY_ENV = 'OPENROUTER_API_KEY'

export const BUDGET_TOKENS = 300_000
export const CAPS = {
  body: 20_000,
  changelog: 8_000,
  deterministic: 6_000,
  previous: 6_000,
  filesRows: 400,
  hunksPerFile: 60_000,
  fullFile: 50_000,
  longLine: 1_000,
}
export const XREF = { ids: 30, hitsPerId: 20, bytes: 40_000 }
export const IMPORTERS = { perModule: 6, perFile: 30_000, bytes: 120_000 }
export const FIXED_SHARE = 0.4

export const MAX_FINDINGS = 8
export const MAX_LISTED = 3
export const HUNK_WINDOW = 40
export const ANCHOR_WINDOW = 5
export const MIN_QUOTE_CHARS = 20
/** Flipped only by a calibration PR (bar C1). */
export const GATE_NEEDS_HIGH = true

export const MAX_TOKENS = { review: 48_000, verify: 16_000 }
export const DEBOUNCE_S = 60
export const ATTEMPTS = { review: 3, verify: 2 }
export const COST_CEILING = 0.25
export const CALIBRATION_MODEL_CEILING = 1.0
export const TIMEOUTS = { git: 30_000, grep: 20_000, judgePr: 60_000, gh: 30_000 }
/** Per-attempt limits; injectable for the tests. */
export const LIMITS = {
  firstByteMs: 60_000,
  idleMs: 120_000,
  attemptMs: 360_000,
  marginMs: 15_000,
  minLeftMs: { review: 180_000, verify: 60_000 },
}

export const LISTED_CATEGORIES = ['concurrency', 'fail-open', 'security', 'correctness', 'dead-wiring', 'migration', 'wire-format', 'data-loss', 'repo-rule', 'accessibility']
export const GATING_CATEGORIES = ['concurrency', 'fail-open', 'security', 'correctness', 'data-loss', 'migration', 'wire-format', 'repo-rule', 'dead-wiring']
export const CAPPED_AT_MINOR = ['claim-vs-code', 'test-gap', 'docs-drift', 'performance']
export const CAPPED_AT_MAJOR = ['accessibility']
export const CATEGORIES = [
  'concurrency', 'fail-open', 'security', 'correctness', 'dead-wiring', 'migration', 'wire-format', 'data-loss',
  'repo-rule', 'accessibility', 'performance', 'claim-vs-code', 'docs-drift', 'test-gap',
]
export const RULE_IDS = ['gateway-bypass', 'secret-in-config', 'forced-done', 'none']
const SEVERITY_RANK = { blocker: 0, major: 1, minor: 2 }
const CONFIDENCE_RANK = { high: 0, medium: 1, low: 2 }

/** Excluded from the diff, the full files and the findings. */
export const GENERATED = [
  /^docs\/api\//,
  /^docs\/CLI-REFERENCE\.md$/,
  /^scripts\/skills\/talaria-apps\/references\/skeleton\.md$/,
  /^ui\/src\/routeTree\.gen\.ts$/,
  /^mcp\/dist\//,
  /^fleet\//,
  /(^|\/)Cargo\.lock$/,
  /^bun\.lockb?$/,
  /(^|\/)package-lock\.json$/,
  /^CHANGELOG\.md$/,
]

/** The rules a change is read against, chosen by the paths it touches. A
 *  `#Heading` suffix sends only that section. Authoritative. */
export const RULES_BY_PATH = [
  { re: null, docs: ['AGENTS.md', 'CONTRIBUTING.md'] },
  { re: /^api\//, docs: ['docs/API-CONVENTIONS.md', 'docs/RUST-MIGRATION.md'] },
  { re: /^ui\/src\/server\//, docs: ['docs/API-CONVENTIONS.md'] },
  { re: /^ui\//, docs: ['docs/UI-CONVENTIONS.md'] },
  { re: /\/migrations\/|pg\.ts$|\.sql$/, docs: ['docs/UPDATES.md', 'docs/ARCHITECTURE.md#Data'] },
]
/** Descriptive documents: context, never a finding by themselves. */
export const REFERENCE_BY_PATH = [
  { re: /^\.github\/|^scripts\/(flow-guard|surfaces)\.mjs$|^scripts\/hooks\//, docs: ['docs/BRANCHES.md'] },
  { re: /secret|encrypt|crypto|keyring|permission|authz|credential/i, docs: ['docs/ENCRYPTION.md', 'docs/PERMISSIONS.md', 'docs/AGENT-KEY-MIGRATION.md'] },
  { re: /^desktop\//, docs: ['docs/DESKTOP.md'] },
]
const REFERENCE_TRIM_ORDER = ['docs/DESKTOP.md', 'docs/BRANCHES.md']

const TEST_PATH = /(^|\/)tests?\/|\.test\.|\.spec\.|_test\.rs$/
const SKIP_REASONS_FOR_HEALTH = new Set(['lane', 'no-key', 'superseded', 'empty', 'too-large', 'no-reviewer-on-trusted', 'bootstrap'])
const GATE_MODES = ['off', 'report', 'blockers']

// ── small helpers ───────────────────────────────────────────────────────────

const sha256 = (s) => createHash('sha256').update(s).digest('hex')
const short = (sha) => String(sha ?? '').slice(0, 12)
const bytes = (s) => Buffer.byteLength(String(s ?? ''), 'utf8')
const defaultSleep = (ms, signal) =>
  new Promise((res) => {
    const t = setTimeout(res, ms)
    signal?.addEventListener?.('abort', () => {
      clearTimeout(t)
      res()
    })
  })

/** AbortSignal.any where the runtime has it; a hand-rolled one where not. */
export function anySignal(signals) {
  const list = signals.filter(Boolean)
  if (typeof AbortSignal.any === 'function') return AbortSignal.any(list)
  const c = new AbortController()
  for (const s of list) {
    if (s.aborted) {
      c.abort(s.reason)
      break
    }
    s.addEventListener('abort', () => c.abort(s.reason), { once: true })
  }
  return c.signal
}

/** Tokens, roughly: bytes over 3, or over 2.5 when the text is not mostly ASCII. */
export function estimateTokens(text) {
  const s = String(text ?? '')
  const b = bytes(s)
  if (!b) return 0
  let nonAscii = 0
  for (let i = 0; i < s.length; i++) if (s.charCodeAt(i) > 127) nonAscii++
  return Math.ceil(b / (nonAscii / s.length > 0.02 ? 2.5 : 3))
}

function capText(s, cap, marker = '[truncated]') {
  const t = String(s ?? '')
  if (bytes(t) <= cap) return t
  let cut = Buffer.from(t, 'utf8').subarray(0, cap).toString('utf8')
  cut = cut.replace(/�+$/, '')
  return `${cut}\n${marker}`
}

const isGeneratedPath = (p) => GENERATED.some((re) => re.test(p))
const isTestPath = (p) => TEST_PATH.test(p)

// ── the key, and every child process ────────────────────────────────────────

/** The environment every child (git, gh, judge-pr) runs with: never a key. */
export function childEnv(extra = {}) {
  const env = {}
  for (const k of ['PATH', 'HOME', 'LANG', 'LC_ALL', 'TMPDIR']) if (process.env[k] !== undefined) env[k] = process.env[k]
  return { ...env, GIT_TERMINAL_PROMPT: '0', GH_PROMPT_DISABLED: '1', ...extra }
}

/** Read the key once, then delete every model key from this process's env
 *  before any child starts. */
export function takeKey(name, env = process.env) {
  const key = String(env[name] ?? '').trim()
  for (const k of [KEY_ENV, CALIBRATION_KEY_ENV, APP_KEY_ENV]) delete env[k]
  return key || null
}

class GitError extends Error {}

/** A git runner bound to a repository directory. Throws GitError on failure. */
export function makeGit(cwd = process.cwd()) {
  const run = (args, { timeout = TIMEOUTS.git, buffer = false, allowFail = false } = {}) => {
    try {
      return execFileSync('git', args, {
        cwd,
        env: childEnv(),
        encoding: buffer ? 'buffer' : 'utf8',
        stdio: ['ignore', 'pipe', 'pipe'],
        timeout,
        maxBuffer: 256 * 1024 * 1024,
      })
    } catch (e) {
      if (allowFail) return null
      const err = e?.stderr ? safeTerminal(String(e.stderr), 400) : e?.code === 'ETIMEDOUT' ? 'timed out' : String(e?.message ?? e)
      throw new GitError(`git ${args.slice(0, 3).join(' ')}: ${err.trim()}`)
    }
  }
  const git = (args, opts) => {
    const out = run(args, opts)
    return out === null ? null : opts?.buffer ? out : String(out).replace(/\n$/, '')
  }
  git.cwd = cwd
  git.resolves = (ref) => run(['rev-parse', '--verify', '--quiet', `${ref}^{commit}`], { allowFail: true }) !== null
  const blobs = new Map()
  /** A blob as a Buffer, or null when the path is not in that commit. */
  git.blob = (sha, path) => {
    const key = `${sha}\0${path}`
    if (!blobs.has(key)) blobs.set(key, run(['cat-file', 'blob', `${sha}:${path}`], { buffer: true, allowFail: true }))
    return blobs.get(key)
  }
  return git
}

function gh(args, { timeout = TIMEOUTS.gh, cwd = process.cwd() } = {}) {
  const r = spawnSync('gh', args, {
    cwd,
    encoding: 'utf8',
    timeout,
    maxBuffer: 256 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: childEnv(process.env.GH_TOKEN ? { GH_TOKEN: process.env.GH_TOKEN } : {}),
  })
  return { ok: !r.error && r.status === 0, out: r.stdout ?? '', err: safeTerminal(String(r.stderr ?? r.error?.message ?? ''), 600).trim() }
}

const isBinary = (buf) => Boolean(buf) && buf.subarray(0, 8192).includes(0)

// ── progress (stderr) ───────────────────────────────────────────────────────

let QUIET = false
/** Silence the progress lines (calibration's pool, the tests). */
export function quiet(on = true) {
  QUIET = on
}
function progress(word, text) {
  if (QUIET) return
  process.stderr.write(`judge-review: ${word} — ${escapeData(safeTerminal(text, 2000))}\n`)
}

// ── arguments ───────────────────────────────────────────────────────────────

export const COMMANDS = ['review', 'fetch-prior', 'calibrate', 'score', 'ledger', 'canary']
const VALUE_FLAGS = new Set([
  '--base', '--head', '--target', '--source', '--rules-from', '--pr', '--pr-json', '--comments-json', '--pr-body', '--title',
  '--prior', '--event', '--action', '--out', '--print-prompt', '--model', '--budget-tokens', '--deadline', '--record', '--replay',
  '--repo', '--run-id', '--trusted', '--set', '--only', '--samples', '--concurrency', '--results', '--ledger', '--grades', '--since',
])
const BOOL_FLAGS = new Set(['--github', '--json', '--dry-run', '--bootstrap', '--no-verify', '--escapes', '--health', '--help', '-h'])

/** `{command, opts}` or `{error}`. */
export function parseArgs(argv) {
  const opts = {}
  let command = 'review'
  let i = 0
  if (argv[0] && !argv[0].startsWith('-')) {
    if (!COMMANDS.includes(argv[0])) return { error: `unknown command ${argv[0]}` }
    command = argv[0]
    i = 1
  }
  for (; i < argv.length; i++) {
    const a = argv[i]
    if (VALUE_FLAGS.has(a)) {
      const v = argv[i + 1]
      if (v === undefined) return { error: `${a} needs a value` }
      opts[a.slice(2)] = v
      i++
    } else if (BOOL_FLAGS.has(a)) opts[a.replace(/^-+/, '')] = true
    else return { error: `unknown argument ${a}` }
  }
  return { command, opts }
}

// ── lanes, trust, gate mode ─────────────────────────────────────────────────

/** 'skip' for the lanes whose changes were each reviewed already (rc→main,
 *  promote/*→rc, backflow/*→testing, sync/*→rc, release/*→rc), 'hotfix' for a
 *  hotfix/* into rc, else 'feature'. A fork-qualified source is never a lane. */
export function laneOf(target, source) {
  const t = String(target ?? '')
  const s = String(source ?? '')
  if (!t || !s || /^[^/:\s]+\/[^/:\s]+:/.test(s)) return 'feature'
  if (t === 'main' && s === 'rc') return 'skip'
  if (t === 'rc' && /^(promote|sync|release)\//.test(s)) return 'skip'
  if (t === 'testing' && s.startsWith('backflow/')) return 'skip'
  if (t === 'rc' && s.startsWith('hotfix/')) return 'hotfix'
  return 'feature'
}

/** surfaces.mjs's pickBase speaks of HEAD; the review's head may not be the
 *  checkout's. The literal argument HEAD, and a range ending in `..HEAD`,
 *  become the head sha. */
export function headWrapped(args, head) {
  return args.map((a) => (a === 'HEAD' ? head : String(a).replace(/\.\.HEAD$/, `..${head}`)))
}

/** The trusted ref: the target when it is a protected branch, else testing
 *  when this clone has it, else rc — the same rule judge-review.yml uses. */
export function trustedRefFor(target, resolves) {
  if (['testing', 'rc', 'main'].includes(target)) return `origin/${target}`
  return resolves('origin/testing') ? 'origin/testing' : 'origin/rc'
}

export function gateModeOf(env = process.env) {
  const v = String(env.JUDGE_REVIEW_GATE ?? '').trim()
  return GATE_MODES.includes(v) ? v : 'report'
}

// ── the range ───────────────────────────────────────────────────────────────

/** The changed files, `mergeBase..head`: [{path, oldPath, status, added, deleted}]. */
export function changedFiles(git, mergeBase, head) {
  const DIFF = ['--text', '--no-textconv', '--no-ext-diff', '--find-renames']
  const ns = git(['diff', '--name-status', '-z', ...DIFF, mergeBase, head]).split('\0')
  const files = []
  for (let i = 0; i < ns.length; ) {
    const status = ns[i]
    if (!status) {
      i++
      continue
    }
    if (/^[RC]/.test(status)) {
      files.push({ status: status[0], oldPath: ns[i + 1], path: ns[i + 2], added: 0, deleted: 0 })
      i += 3
    } else {
      files.push({ status: status[0], oldPath: null, path: ns[i + 1], added: 0, deleted: 0 })
      i += 2
    }
  }
  const byPath = new Map(files.map((f) => [f.path, f]))
  const num = git(['diff', '--numstat', '-z', ...DIFF, mergeBase, head]).split('\0')
  for (let i = 0; i < num.length; ) {
    const row = num[i]
    if (!row) {
      i++
      continue
    }
    const m = /^(\d+|-)\t(\d+|-)\t(.*)$/.exec(row)
    if (!m) {
      i++
      continue
    }
    let path = m[3]
    if (path === '') {
      path = num[i + 2]
      i += 3
    } else i += 1
    const f = byPath.get(path)
    if (f) {
      f.added = Number(m[1]) || 0
      f.deleted = Number(m[2]) || 0
    }
  }
  return files
}

/** The new-side lines a diff of one file changes: added lines, both sides of
 *  a deletion-only block (c and c+1), and every line of a new file. */
export function changedPoints(diffText, { allLines = 0 } = {}) {
  const points = new Set()
  if (allLines) {
    for (let n = 1; n <= allLines; n++) points.add(n)
    return points
  }
  let newLine = 0
  let inHunk = false
  let run = { minus: 0, plus: 0 }
  const flush = () => {
    if (run.minus > 0 && run.plus === 0) {
      points.add(Math.max(1, newLine - 1))
      points.add(newLine)
    }
    run = { minus: 0, plus: 0 }
  }
  for (const l of String(diffText).split('\n')) {
    const h = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@/.exec(l)
    if (h) {
      flush()
      inHunk = true
      newLine = Number(h[1])
      // A deletion-only hunk at -U0: `+c,0` — c is the line before the removal.
      if (h[2] === '0') newLine = Number(h[1]) + 1
      continue
    }
    if (!inHunk) continue
    if (l.startsWith('+')) {
      run.plus++
      points.add(newLine++)
    } else if (l.startsWith('-')) run.minus++
    else if (l.startsWith(' ')) {
      flush()
      newLine++
    } else if (l.startsWith('\\')) continue
    else {
      flush()
      inHunk = false
    }
  }
  flush()
  return points
}

/** The diff text from the first hunk on (headers dropped), capped at hunk boundaries. */
export function capHunks(diffText, cap) {
  const at = diffText.indexOf('\n@@')
  const body = at === -1 ? (diffText.startsWith('@@') ? diffText : '') : diffText.slice(at + 1)
  if (bytes(body) <= cap) return { text: body, truncated: false }
  const hunks = body.split(/\n(?=@@ )/)
  const kept = []
  let size = 0
  for (const h of hunks) {
    if (size + bytes(h) > cap) break
    kept.push(h)
    size += bytes(h) + 1
  }
  if (!kept.length) kept.push(capText(hunks[0], cap, ''))
  return { text: `${kept.join('\n')}\n[hunks truncated]`, truncated: true }
}

/** Risk score of a file, for trimming: 3 product source, 2 build and
 *  workflow, 1 tests, 0 docs. */
export function riskOf(path) {
  if (/\.(md|txt)$/i.test(path) || /^docs\//.test(path)) return 0
  if (isTestPath(path)) return 1
  if (/^api\/crates\/.*\/src\/.*\.rs$/.test(path) || /^(ui|cli|mcp|desktop)\//.test(path)) return 3
  if (/\/migrations\/|^\.github\/workflows\/|^scripts\/[^/]+\.mjs$|(^|\/)Dockerfile/.test(path)) return 2
  return 2
}

// ── documents ───────────────────────────────────────────────────────────────

/** A section of a markdown document by its heading text, up to the next
 *  heading of the same level or higher. null when it is absent. */
export function extractSection(text, heading) {
  const lines = String(text ?? '').split('\n')
  let level = 0
  let start = -1
  for (let i = 0; i < lines.length; i++) {
    const m = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(lines[i])
    if (!m) continue
    if (start === -1) {
      if (m[2] === heading) {
        start = i
        level = m[1].length
      }
    } else if (m[1].length <= level) return lines.slice(start, i).join('\n').trim()
  }
  return start === -1 ? null : lines.slice(start).join('\n').trim()
}

/** The RULES and REFERENCE doc keys (path, or path#Section) for these paths. */
export function docsFor(paths) {
  const pick = (table) => {
    const out = []
    for (const { re, docs } of table) {
      if (re === null || paths.some((p) => re.test(p))) for (const d of docs) if (!out.includes(d)) out.push(d)
    }
    return out
  }
  const rules = pick(RULES_BY_PATH)
  const reference = pick(REFERENCE_BY_PATH).filter((d) => !rules.includes(d))
  return { rules, reference }
}

function readDoc(git, sha, key) {
  const [path, heading] = key.split('#')
  const buf = git.blob(sha, path)
  if (!buf) return null
  const text = buf.toString('utf8')
  if (!heading) return { path: key, text }
  const sec = extractSection(text, heading)
  return sec ? { path: key, text: sec } : null
}

// ── the prompt ──────────────────────────────────────────────────────────────

/** The prompt file's `## system`, `## review` and `## verify` sections. */
export function parsePrompt(text) {
  const out = {}
  const parts = String(text).split(/^## /m)
  for (const part of parts.slice(1)) {
    const nl = part.indexOf('\n')
    const name = part.slice(0, nl).trim()
    out[name] = part.slice(nl + 1).trim()
  }
  if (!out.system || !out.review || !out.verify) throw new Error('the prompt file lacks a ## system, ## review or ## verify section')
  return out
}

export function reviewerVersion(dir = SCRIPT_DIR) {
  const h = createHash('sha256')
  for (const f of ['judge-review.mjs', 'judge-review-wire.mjs', 'judge-review.prompt.md']) {
    try {
      h.update(readFileSync(join(dir, f)))
    } catch {
      h.update(`missing:${f}`)
    }
  }
  return h.digest('hex')
}

// ── schemas ─────────────────────────────────────────────────────────────────

const str = (description) => ({ type: 'string', description })
export const REVIEW_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['summary', 'findings', 'verification_gap', 'could_not_judge'],
  properties: {
    summary: str('at most two sentences saying what the change does; no judgement'),
    findings: {
      type: 'array',
      description: 'at most 8, most severe first; an empty list is a good review',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['severity', 'category', 'rule_id', 'file', 'line', 'quote', 'title', 'failure', 'fix', 'rule', 'confidence'],
        properties: {
          severity: { type: 'string', enum: ['blocker', 'major', 'minor'], description: 'blocker, major or minor as the review section defines them' },
          category: { type: 'string', enum: CATEGORIES, description: 'the defect class' },
          rule_id: { type: 'string', enum: RULE_IDS, description: 'the AGENTS.md rule this breaks, or none' },
          file: str('repo-relative path exactly as in FILES'),
          line: { type: 'integer', description: 'new-file line number of the first quoted line' },
          quote: str('1 to 5 consecutive lines of the new file, copied exactly, without line-number prefixes or diff markers'),
          title: str('one short sentence naming the defect'),
          failure: str('the input or state, the path to the quoted code, and the wrong result'),
          fix: str('the smallest change that removes the failure'),
          rule: str("doc path and section, or 'none'"),
          confidence: { type: 'string', enum: ['high', 'medium', 'low'], description: 'high: traced in the code shown; medium: likely; low: discarded' },
        },
      },
    },
    verification_gap: str('one sentence when the stated verification misses a visible risk, else empty'),
    could_not_judge: { type: 'array', description: 'what you would need to decide a suspicion', items: str('one item') },
  },
}
export const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['verdicts'],
  properties: {
    verdicts: {
      type: 'array',
      description: 'exactly one per candidate, by index',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['index', 'verdict', 'input', 'trace', 'evidence'],
        properties: {
          index: { type: 'integer', description: 'the candidate number' },
          verdict: { type: 'string', enum: ['confirmed', 'refuted', 'unsure'], description: 'confirmed, refuted or unsure' },
          input: str('the triggering input or state; empty unless confirmed'),
          trace: str('the path from the input to the wrong result; empty unless confirmed'),
          evidence: str('one or two sentences naming the decisive line or fact'),
        },
      },
    },
  },
}

/** The review output's top-level shape. Findings are checked one by one later. */
export function validateReview(text) {
  let v
  try {
    v = JSON.parse(String(text).trim().replace(/^```(?:json)?\s*|\s*```$/g, ''))
  } catch (e) {
    return { ok: false, error: `not JSON (${e.message})` }
  }
  if (!v || typeof v !== 'object' || Array.isArray(v)) return { ok: false, error: 'not an object' }
  if (!Array.isArray(v.findings)) return { ok: false, error: 'findings is not a list' }
  return {
    ok: true,
    value: {
      summary: typeof v.summary === 'string' ? v.summary : '',
      findings: v.findings,
      verification_gap: typeof v.verification_gap === 'string' ? v.verification_gap : '',
      could_not_judge: Array.isArray(v.could_not_judge) ? v.could_not_judge.filter((x) => typeof x === 'string') : [],
    },
  }
}

export function validateVerify(text, count) {
  let v
  try {
    v = JSON.parse(String(text).trim().replace(/^```(?:json)?\s*|\s*```$/g, ''))
  } catch (e) {
    return { ok: false, error: `not JSON (${e.message})` }
  }
  if (!v || !Array.isArray(v.verdicts)) return { ok: false, error: 'verdicts is not a list' }
  const out = new Array(count).fill(null)
  for (const r of v.verdicts) {
    if (!r || !Number.isInteger(r.index) || r.index < 0 || r.index >= count) continue
    if (!['confirmed', 'refuted', 'unsure'].includes(r.verdict)) continue
    if (out[r.index]) continue
    out[r.index] = {
      verdict: r.verdict,
      input: typeof r.input === 'string' ? r.input : '',
      trace: typeof r.trace === 'string' ? r.trace : '',
      evidence: typeof r.evidence === 'string' ? r.evidence : '',
    }
  }
  return { ok: true, value: out.map((r) => r ?? { verdict: 'unsure', input: '', trace: '', evidence: 'no verdict returned' }) }
}

// ── the request ─────────────────────────────────────────────────────────────

export function requestBody({ model, effort = 'high', purpose, messages }) {
  const provider = {
    require_parameters: true,
    allow_fallbacks: true,
    data_collection: 'deny',
    zdr: true,
    quantizations: ['fp8', 'fp16', 'bf16', 'unknown'],
  }
  if (PRICE_CAPS[model]) provider.max_price = { ...PRICE_CAPS[model] }
  return {
    model,
    stream: true,
    max_tokens: MAX_TOKENS[purpose],
    reasoning: { effort, exclude: false },
    provider,
    response_format: {
      type: 'json_schema',
      json_schema: { name: purpose === 'review' ? 'pr_review' : 'pr_verify', strict: true, schema: purpose === 'review' ? REVIEW_SCHEMA : VERIFY_SCHEMA },
    },
    // Usage accounting: the final chunk carries cost, which the ceiling needs.
    usage: { include: true },
    messages,
  }
}

export function requestHeaders(key) {
  return {
    Authorization: `Bearer ${key}`,
    'Content-Type': 'application/json',
    'HTTP-Referer': 'https://github.com/outcrop-labs/talaria',
    'X-OpenRouter-Title': 'talaria judge',
    'X-Title': 'talaria judge',
    'X-OpenRouter-App-Visibility': 'hidden',
  }
}

// ── the stream ──────────────────────────────────────────────────────────────

async function* chunksOf(readable) {
  if (!readable) return
  if (typeof readable === 'string') {
    yield new TextEncoder().encode(readable)
    return
  }
  if (typeof readable.getReader === 'function') {
    const reader = readable.getReader()
    try {
      for (;;) {
        const { done, value } = await reader.read()
        if (done) return
        yield value
      }
    } finally {
      reader.releaseLock?.()
    }
  }
  for await (const c of readable) yield typeof c === 'string' ? new TextEncoder().encode(c) : c
}

/** An OpenRouter SSE stream: the content, the ids, the finish, the usage and
 *  any error chunk. `onActivity` is called on every byte that arrives (a
 *  keep-alive included), so the caller's idle timer can reset. */
export async function parseSse(readable, now = Date.now, onActivity = () => {}) {
  const dec = new TextDecoder('utf-8')
  const out = { content: '', refusal: '', id: null, model: null, provider: null, finish: null, usage: null, badChunks: 0, error: null, firstByteAt: null, firstContentAt: null, lastByteAt: null, data: [], done: false }
  let buf = ''
  const line = (raw) => {
    const l = raw.endsWith('\r') ? raw.slice(0, -1) : raw
    if (!l || l.startsWith(':')) return
    if (!l.startsWith('data:')) return
    const data = l.slice(5).trim()
    if (data === '[DONE]') {
      out.done = true
      return
    }
    out.data.push(data)
    let j
    try {
      j = JSON.parse(data)
    } catch {
      out.badChunks++
      return
    }
    if (j.id) out.id = String(j.id)
    if (j.model) out.model = String(j.model)
    if (j.provider) out.provider = String(j.provider)
    if (j.error) out.error = { type: String(j.error.metadata?.error_type ?? j.error.code ?? 'error'), message: String(j.error.message ?? '') }
    const ch = Array.isArray(j.choices) ? j.choices[0] : null
    if (ch) {
      const c = ch.delta?.content
      if (typeof c === 'string' && c) {
        if (out.firstContentAt === null) out.firstContentAt = now()
        out.content += c
      }
      if (typeof ch.delta?.refusal === 'string') out.refusal += ch.delta.refusal
      if (ch.finish_reason) out.finish = String(ch.finish_reason)
      if (ch.finish_reason === 'error' && !out.error) out.error = { type: String(ch.error?.metadata?.error_type ?? 'error'), message: String(ch.error?.message ?? '') }
    }
    if (j.usage) out.usage = j.usage
  }
  for await (const chunk of chunksOf(readable)) {
    const t = now()
    if (out.firstByteAt === null) out.firstByteAt = t
    out.lastByteAt = t
    onActivity()
    buf += dec.decode(chunk, { stream: true })
    let nl
    while ((nl = buf.indexOf('\n')) >= 0) {
      line(buf.slice(0, nl))
      buf = buf.slice(nl + 1)
      if (out.done) break
    }
    if (out.done) break
  }
  buf += dec.decode()
  if (buf && !out.done) line(buf)
  return out
}

/** The recorded stream: reasoning removed from every chunk, no headers, no key. */
export function recordable(data) {
  const lines = []
  for (const d of data) {
    let j
    try {
      j = JSON.parse(d)
    } catch {
      continue
    }
    for (const ch of Array.isArray(j.choices) ? j.choices : []) {
      if (ch?.delta) for (const k of Object.keys(ch.delta)) if (k.startsWith('reasoning')) delete ch.delta[k]
      if (ch?.message) for (const k of Object.keys(ch.message)) if (k.startsWith('reasoning')) delete ch.message[k]
    }
    for (const k of Object.keys(j)) if (k.startsWith('reasoning')) delete j[k]
    lines.push(`data: ${JSON.stringify(j)}\n`)
  }
  lines.push('data: [DONE]\n')
  return lines.join('\n')
}

/** A fetch that answers from recorded files, `<dir>/<purpose>-<n>.sse` (a
 *  200 stream) or `.json` ({status, headers, body}). Test-only. */
export function replayFetch(dir) {
  const seen = { review: 0, verify: 0 }
  return async (_url, init) => {
    let purpose = 'review'
    try {
      purpose = JSON.parse(init.body).response_format.json_schema.name === 'pr_verify' ? 'verify' : 'review'
    } catch {
      // the review, by default
    }
    const n = ++seen[purpose]
    const sse = join(dir, `${purpose}-${n}.sse`)
    const js = join(dir, `${purpose}-${n}.json`)
    if (existsSync(sse)) return fakeResponse(200, {}, readFileSync(sse, 'utf8'))
    if (existsSync(js)) {
      const r = JSON.parse(readFileSync(js, 'utf8'))
      return fakeResponse(r.status ?? 200, r.headers ?? {}, typeof r.body === 'string' ? r.body : JSON.stringify(r.body ?? ''))
    }
    throw new Error(`network refused (no recording ${purpose}-${n} in the replay directory)`)
  }
}

export function fakeResponse(status, headers, body) {
  const h = new Map(Object.entries(headers).map(([k, v]) => [k.toLowerCase(), String(v)]))
  return {
    status,
    ok: status >= 200 && status < 300,
    headers: { get: (k) => h.get(String(k).toLowerCase()) ?? null },
    body: (async function* () {
      yield new TextEncoder().encode(body)
    })(),
    text: async () => body,
  }
}

// ── the call ────────────────────────────────────────────────────────────────

class OfflineError extends Error {}

/** A run's call budget: the billed attempts per purpose, the summed cost,
 *  the ceiling, and every attempt's record. */
export function callState(ceiling = COST_CEILING) {
  return { billed: { review: 0, verify: 0 }, cost: 0, ceiling, calls: [], ceilingHit: false }
}

const RETRYABLE_ERROR_TYPES = new Set(['rate_limit_exceeded', 'provider_overloaded', 'provider_unavailable', 'timeout', 'server', 'server_error'])

function retryAfterS(res) {
  const v = res?.headers?.get?.('retry-after')
  if (!v) return null
  const n = Number(v)
  if (Number.isFinite(n)) return n
  const t = Date.parse(v)
  return Number.isFinite(t) ? Math.max(0, (t - Date.now()) / 1000) : null
}

/**
 * One model request, with the retries §2.8 of the design allows. Returns
 * `{ok: true, value, content, call}` or `{ok: false, reason, text, raw}`.
 * `validate(content)` → `{ok, value, error}`. Throws OfflineError when the
 * network is refused. Every attempt is recorded in `state.calls`.
 */
export async function callModel({
  body,
  key,
  purpose,
  state,
  validate,
  fetchImpl = globalThis.fetch,
  sleep = defaultSleep,
  now = Date.now,
  deadlineAt,
  runSignal,
  limits = LIMITS,
  onRecord = null,
}) {
  if (!key) return { ok: false, reason: 'no-key', text: `no ${KEY_ENV}` }
  let effort = body.reasoning?.effort ?? 'high'
  const used = { low: false, errorChunk: false, invalid: false, credit: false, capLow: false }
  let last = { reason: 'provider-error', text: '' }
  let retryWait = null
  for (;;) {
    if (state.billed[purpose] >= ATTEMPTS[purpose]) return { ok: false, ...last }
    if (retryWait !== null) {
      await sleep(retryWait * 1000, runSignal)
      retryWait = null
    }
    if (runSignal?.aborted) return { ok: false, reason: 'timeout', text: 'the run was stopped' }
    const left = deadlineAt - now()
    if (left < limits.minLeftMs[purpose]) return { ok: false, reason: 'timeout', text: `${Math.max(0, Math.round(left / 1000))}s left before the deadline — not enough to start a ${purpose} call` }
    if (state.cost >= state.ceiling) {
      state.ceilingHit = true
      return { ok: false, reason: 'cost-ceiling', text: `the run has spent $${state.cost.toFixed(4)} of its $${state.ceiling.toFixed(2)} ceiling` }
    }
    state.billed[purpose]++
    const attemptMs = Math.min(limits.attemptMs, left - limits.marginMs)
    const capped = attemptMs >= limits.attemptMs
    const call = { purpose, attempt: state.billed[purpose], effort, status: null, generationId: null, provider: null, model: null, promptTokens: null, completionTokens: null, reasoningTokens: null, cachedTokens: null, tokensPerSec: null, cost: null, ms: 0, finish: null, errorType: null }
    state.calls.push(call)
    const started = now()
    const attemptCtl = new AbortController()
    const attemptTimer = setTimeout(() => attemptCtl.abort(new Error('attempt-cap')), Math.max(1, attemptMs))
    const idleCtl = new AbortController()
    let idleTimer = setTimeout(() => idleCtl.abort(new Error('idle')), limits.firstByteMs)
    const activity = () => {
      clearTimeout(idleTimer)
      idleTimer = setTimeout(() => idleCtl.abort(new Error('idle')), limits.idleMs)
    }
    const signal = anySignal([attemptCtl.signal, idleCtl.signal, runSignal])
    const reqBody = { ...body, reasoning: { ...body.reasoning, effort } }
    let res
    let stream
    let netError = null
    try {
      res = await raceSignal(fetchImpl(ENDPOINT, { method: 'POST', headers: requestHeaders(key), body: JSON.stringify(reqBody), signal }), signal)
      call.status = res.status
      if (res.status === 200) {
        activity()
        stream = await raceSignal(parseSse(res.body, now, activity), signal)
      }
    } catch (e) {
      if (e instanceof OfflineError || /network refused/.test(String(e?.message))) {
        clearTimeout(attemptTimer)
        clearTimeout(idleTimer)
        call.status = 'offline'
        throw e instanceof OfflineError ? e : new OfflineError(String(e.message))
      }
      netError = e
    } finally {
      clearTimeout(attemptTimer)
      clearTimeout(idleTimer)
    }
    call.ms = now() - started
    const whyAborted = signal.aborted ? String(signal.reason?.message ?? signal.reason ?? '') : ''

    if (runSignal?.aborted) return { ok: false, reason: 'timeout', text: 'the run was stopped' }
    if (netError || !res) {
      call.errorType = whyAborted || 'network'
      if (whyAborted === 'attempt-cap') {
        if (capped && !used.capLow) {
          used.capLow = true
          effort = 'low'
          last = { reason: 'timeout', text: `the attempt reached ${Math.round(limits.attemptMs / 1000)}s` }
          continue
        }
        last = { reason: 'timeout', text: 'the attempt ran out of time' }
        return { ok: false, ...last }
      }
      last = { reason: whyAborted === 'idle' ? 'timeout' : 'provider-error', text: whyAborted === 'idle' ? 'the stream stalled' : `network: ${safeTerminal(String(netError?.message ?? netError), 200)}` }
      retryWait = state.billed[purpose] === 1 ? 5 : 15
      continue
    }

    if (res.status !== 200) {
      const text = await res.text().catch(() => '')
      let err = {}
      try {
        err = JSON.parse(text)?.error ?? {}
      } catch {
        // not JSON
      }
      const msg = safeTerminal(String(err.message ?? text), 300)
      call.errorType = String(err.metadata?.error_type ?? err.code ?? res.status)
      const ra = retryAfterS(res)
      if (res.status === 402 || (res.status === 403 && err.metadata?.limit_source === 'openrouter_in_flight_budget')) {
        if (!used.credit && ra !== null && ra <= 30) {
          used.credit = true
          retryWait = ra
          last = { reason: 'out-of-credit', text: msg }
          continue
        }
        return { ok: false, reason: 'out-of-credit', text: msg }
      }
      if (res.status === 400) return { ok: false, reason: 'request-rejected', text: msg }
      if (res.status === 401 || res.status === 403) return { ok: false, reason: 'key-rejected', text: msg }
      if (res.status === 404) return { ok: false, reason: 'no-endpoint', text: `${msg} (model ${body.model}; provider filters: zdr, data_collection=deny, quantizations ${body.provider?.quantizations?.join('/')}, require_parameters)` }
      if (res.status === 408 || res.status === 429 || res.status >= 500) {
        last = { reason: res.status === 408 ? 'timeout' : 'provider-error', text: `HTTP ${res.status}: ${msg}` }
        retryWait = ra !== null && ra <= 30 ? ra : state.billed[purpose] === 1 ? 5 : 15
        continue
      }
      return { ok: false, reason: 'request-rejected', text: `HTTP ${res.status}: ${msg}` }
    }

    // A 200: the stream.
    call.generationId = stream.id
    call.provider = stream.provider ? slug(stream.provider) : null
    call.model = stream.model ? slug(stream.model) : null
    call.finish = stream.finish
    const u = stream.usage ?? {}
    call.promptTokens = u.prompt_tokens ?? null
    call.completionTokens = u.completion_tokens ?? null
    call.reasoningTokens = u.completion_tokens_details?.reasoning_tokens ?? null
    call.cachedTokens = u.prompt_tokens_details?.cached_tokens ?? null
    if (typeof u.cost === 'number' && Number.isFinite(u.cost)) call.cost = u.cost
    else if (call.promptTokens !== null && PRICE_CAPS[body.model]) {
      call.cost = (call.promptTokens * PRICE_CAPS[body.model].prompt + (call.completionTokens ?? 0) * PRICE_CAPS[body.model].completion) / 1e6
      call.costEstimated = true
    }
    if (call.cost !== null) state.cost += call.cost
    const secs = ((stream.lastByteAt ?? now()) - (stream.firstContentAt ?? stream.firstByteAt ?? started)) / 1000
    if (call.completionTokens && secs > 0) call.tokensPerSec = Math.round((call.completionTokens / secs) * 10) / 10
    if (onRecord) onRecord(purpose, call.attempt, stream.data)

    if (whyAborted) {
      call.errorType = whyAborted
      if (whyAborted === 'attempt-cap' && capped && !used.capLow) {
        used.capLow = true
        effort = 'low'
        last = { reason: 'timeout', text: 'the attempt reached its cap' }
        continue
      }
      last = { reason: 'timeout', text: whyAborted === 'idle' ? 'the stream stalled' : 'the attempt ran out of time' }
      if (whyAborted === 'idle') {
        retryWait = 5
        continue
      }
      return { ok: false, ...last }
    }
    if (stream.error) {
      call.errorType = stream.error.type
      if (!used.errorChunk && RETRYABLE_ERROR_TYPES.has(stream.error.type)) {
        used.errorChunk = true
        last = { reason: 'provider-error', text: `${stream.error.type}: ${safeTerminal(stream.error.message, 200)}` }
        retryWait = 5
        continue
      }
      return { ok: false, reason: 'provider-error', text: `${stream.error.type}: ${safeTerminal(stream.error.message, 200)}` }
    }
    if (stream.finish === 'content_filter' || stream.refusal) return { ok: false, reason: 'refused', text: 'the model refused or a content filter stopped it' }
    const checked = validate(stream.content)
    const visible = (call.completionTokens ?? 0) - (call.reasoningTokens ?? 0)
    if (stream.finish === 'length' || (!checked.ok && visible < 50)) {
      if (!used.low) {
        used.low = true
        effort = 'low'
        last = { reason: 'truncated', text: `finish ${stream.finish ?? 'none'}, ${visible} visible tokens` }
        continue
      }
      return { ok: false, reason: 'truncated', text: `finish ${stream.finish ?? 'none'}, ${visible} visible tokens`, raw: stream.content }
    }
    if (!checked.ok) {
      if (!used.invalid) {
        used.invalid = true
        last = { reason: 'invalid-output', text: checked.error, raw: stream.content }
        continue
      }
      return { ok: false, reason: 'invalid-output', text: checked.error, raw: stream.content }
    }
    return { ok: true, value: checked.value, content: stream.content, call }
  }
}

function raceSignal(promise, signal) {
  if (!signal) return promise
  return new Promise((res, rej) => {
    if (signal.aborted) return rej(signal.reason ?? new Error('aborted'))
    const onAbort = () => rej(signal.reason ?? new Error('aborted'))
    signal.addEventListener('abort', onAbort, { once: true })
    Promise.resolve(promise).then(
      (v) => {
        signal.removeEventListener('abort', onAbort)
        res(v)
      },
      (e) => {
        signal.removeEventListener('abort', onAbort)
        rej(e)
      },
    )
  })
}

// ── cross-references and importers ──────────────────────────────────────────

const CALL_STOP = new Set(
  ('if for while match return switch catch function some none self super new await async typeof instanceof delete void ' +
    'println print eprintln format write writeln assert assert_eq assert_ne debug_assert expect unwrap unwrap_or unwrap_or_else unwrap_or_default ' +
    'into from clone len iter into_iter collect filter map flat_map and_then ok_or ok_or_else is_some is_none is_ok is_err as_ref as_str ' +
    'to_string to_owned push insert remove get set join split slice includes length trim then catch finally resolve reject ' +
    'console log error warn info trace require import export const let var test describe it expect each before after beforeEach afterEach ' +
    'string number boolean object array json parse stringify keys values entries assign freeze from of has add sort reverse concat find ' +
    'replace match exec startsWith endsWith padStart toFixed toString valueOf apply call bind fetch setTimeout clearTimeout ' +
    'Some None Ok Err Self Vec Box Arc Rc String Option Result HashMap HashSet BTreeMap Promise Object Array JSON Math Number Date Error Map Set')
    .split(/\s+/)
    .map((s) => s.toLowerCase()),
)

/** The identifiers a diff puts in play: calls it removes that no added line
 *  makes again (`removed-call`), and the items it adds (`added-item`), with
 *  where each added item is defined. Removed calls first; 30 at most. */
export function identifiersFromDiffs(fileDiffs) {
  const added = []
  const removedCalls = new Map()
  const addedItems = new Map()
  for (const { path, text } of fileDiffs) {
    let newLine = 0
    let inHunk = false
    for (const l of String(text).split('\n')) {
      const h = /^@@ -\d+(?:,\d+)? \+(\d+)/.exec(l)
      if (h) {
        newLine = Number(h[1])
        inHunk = true
        continue
      }
      if (!inHunk) continue
      if (l.startsWith('+')) {
        const body = l.slice(1)
        added.push(body)
        const items = []
        for (const m of body.matchAll(/(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([a-z_][a-z0-9_]{2,})/g)) items.push(m[1])
        for (const m of body.matchAll(/pub\s+(?:struct|enum|trait|const|static)\s+(\w{3,})/g)) items.push(m[1])
        if (/\.(ts|js|mjs|svelte)$/.test(path)) {
          for (const m of body.matchAll(/export (?:async )?function (\w{3,})/g)) items.push(m[1])
          for (const m of body.matchAll(/export (?:const|let|class) (\w{3,})/g)) items.push(m[1])
        }
        for (const id of items) if (!addedItems.has(id)) addedItems.set(id, `${path}:${newLine}`)
        newLine++
      } else if (l.startsWith('-')) {
        for (const m of l.slice(1).matchAll(/([A-Za-z_]\w{3,})\s*\(/g)) {
          const id = m[1]
          if (CALL_STOP.has(id.toLowerCase())) continue
          if (/^(fn|function)\s/.test(l.slice(1).trim()) && new RegExp(`(fn|function)\\s+${id}\\b`).test(l)) continue
          if (!removedCalls.has(id)) removedCalls.set(id, path)
        }
      } else if (l.startsWith(' ')) newLine++
    }
  }
  const addedText = added.join('\n')
  const out = []
  for (const [id] of removedCalls) {
    if (new RegExp(`\\b${id}\\s*\\(`).test(addedText)) continue
    out.push({ id, kind: 'removed-call', definedAt: null })
  }
  for (const [id, at] of addedItems) if (!out.some((x) => x.id === id)) out.push({ id, kind: 'added-item', definedAt: at })
  return out.slice(0, XREF.ids)
}

const DEF_RE = (id) => new RegExp(`\\b(fn|function|struct|enum|trait|const|static|class|type|interface|let|var|def)\\s+${id}\\b`)

/** Where each identifier is used at head, test and non-test, by git grep. */
export function crossReferences(git, head, ids, { isExcluded = () => false } = {}) {
  const cfgTest = new Map()
  const cfgTestLine = (path) => {
    if (cfgTest.has(path)) return cfgTest.get(path)
    if (cfgTest.size >= 50) return Infinity
    const buf = git.blob(head, path)
    const lines = buf ? buf.toString('utf8').split('\n') : []
    const at = lines.findIndex((l) => l.includes('#[cfg(test)]'))
    const v = at === -1 ? Infinity : at + 1
    cfgTest.set(path, v)
    return v
  }
  const results = []
  let used = 0
  for (const x of ids) {
    let raw = null
    try {
      raw = git(['grep', '-a', '-n', '-z', '-w', '-F', '-e', x.id, head, '--'], { timeout: TIMEOUTS.grep, allowFail: true })
    } catch {
      raw = null
    }
    const hits = []
    for (const row of String(raw ?? '').split('\n')) {
      if (!row) continue
      const parts = row.split('\0')
      if (parts.length < 3) continue
      const path = parts[0].slice(parts[0].indexOf(':') + 1)
      const line = Number(parts[1])
      const text = parts.slice(2).join('\0')
      if (isExcluded(path)) continue
      if (isBinary(git.blob(head, path))) continue
      hits.push({ path, line, text: text.trim().slice(0, 200), test: isTestPath(path) || line > cfgTestLine(path), def: DEF_RE(x.id).test(text) })
    }
    const defHit = hits.find((h) => h.def && !h.test)
    const definedAt = x.definedAt ?? (defHit ? `${defHit.path}:${defHit.line}` : null)
    const uses = hits.filter((h) => !(h.def && `${h.path}:${h.line}` === definedAt) && !(x.kind === 'added-item' && h.def))
    const nonTestUses = uses.filter((h) => !h.test).length
    const testUses = uses.filter((h) => h.test).length
    const header = `### ${x.id} — ${x.kind} — defined at ${definedAt ?? 'unknown'}; ${nonTestUses} non-test uses, ${testUses} in tests at head`
    const listing = hits.slice(0, XREF.hitsPerId).map((h) => `${h.path}:${h.line}${h.test ? ' [test]' : ''}: ${h.text}`)
    const text = [header, ...listing, ...(hits.length > XREF.hitsPerId ? [`[${hits.length - XREF.hitsPerId} more hits]`] : [])].join('\n')
    if (used + bytes(text) > XREF.bytes) break
    used += bytes(text)
    results.push({ ...x, definedAt, nonTestUses, testUses, text })
  }
  return results
}

/** The import specifiers of a source file. */
export function importSpecifiers(text) {
  return [...String(text).matchAll(/(?:from|import\()\s*['"]([^'"]+)['"]/g)].map((m) => m[1])
}

/** Does `spec`, imported from `importer`, name `target`? Relative and `$lib` only. */
export function resolvesTo(importer, spec, target) {
  let p
  if (spec.startsWith('$lib/')) p = `ui/src/lib/${spec.slice(5)}`
  else if (spec === '$lib') p = 'ui/src/lib'
  else if (spec.startsWith('.')) p = posix.normalize(posix.join(posix.dirname(importer), spec))
  else return false
  const cands = [p, `${p}.ts`, `${p}.js`, `${p}.svelte`, p.replace(/\.js$/, '.ts'), `${p}/index.ts`, `${p}/index.js`]
  return cands.includes(target)
}

function escapeEre(s) {
  return s.replace(/[.[\]()*+?^${}|\\]/g, '\\$&')
}

/** Files at head that import a changed module (ui, mcp, cli), with their
 *  content, within the caps. */
export function findImporters(git, head, modules, changedSet, { isExcluded = () => false } = {}) {
  const out = []
  let total = 0
  for (const mod of modules) {
    const pkg = mod.split('/')[0]
    const base = posix.basename(mod).replace(/\.(ts|js|mjs|svelte)$/, '')
    const re = `(from|import\\()\\s*['"][^'"]*/${escapeEre(base)}(\\.svelte|\\.js|\\.ts)?['"]`
    const raw = git(['grep', '-l', '-a', '-E', '-e', re, head, '--', `${pkg}/`], { timeout: TIMEOUTS.grep, allowFail: true })
    let n = 0
    for (const row of String(raw ?? '').split('\n')) {
      if (!row || n >= IMPORTERS.perModule) continue
      const path = row.slice(row.indexOf(':') + 1)
      if (changedSet.has(path) || isExcluded(path) || out.some((o) => o.path === path)) continue
      const buf = git.blob(head, path)
      if (!buf || isBinary(buf)) continue
      const text = buf.toString('utf8')
      if (!importSpecifiers(text).some((s) => resolvesTo(path, s, mod))) continue
      const numbered = capText(numberLines(text), IMPORTERS.perFile, '[file truncated]')
      if (total + bytes(numbered) > IMPORTERS.bytes) break
      total += bytes(numbered)
      out.push({ path, module: mod, text: numbered })
      n++
    }
  }
  return out
}

export function numberLines(text) {
  return String(text)
    .split('\n')
    .map((l, i) => `${String(i + 1).padStart(5, '0')}| ${l}`)
    .join('\n')
}

// ── context ─────────────────────────────────────────────────────────────────

/**
 * Everything the model reads, as sections that can be trimmed in a fixed
 * order. Pure over `git` and the inputs; `review()` and the dry run share it.
 */
export function buildContext({ git, head, mergeBase, rulesSha, trustedName, target, source, title, body, authorKind, judgePr, prior, budgetTokens = BUDGET_TOKENS, prompt }) {
  const files = changedFiles(git, mergeBase, head)
  const trims = []
  const info = new Map()
  for (const f of files) {
    const headBuf = f.status === 'D' ? null : git.blob(head, f.path)
    const baseBuf = f.status === 'A' ? null : git.blob(mergeBase, f.oldPath ?? f.path)
    const binary = isBinary(headBuf) || (!headBuf && isBinary(baseBuf))
    let generated = isGeneratedPath(f.path) ? 'generated' : binary ? 'binary' : null
    const headText = headBuf && !binary ? headBuf.toString('utf8') : null
    if (!generated && headText && headText.split('\n').some((l) => l.length > CAPS.longLine)) generated = 'minified'
    info.set(f.path, { ...f, binary, generated, headText, headLines: headText ? headText.split('\n') : null, exists: Boolean(headBuf) })
  }
  // .gitattributes is always shown: it decides how every other diff renders.
  const shown = files.filter((f) => !info.get(f.path).generated || f.path === '.gitattributes')
  const DIFF = ['diff', '--text', '--no-textconv', '--no-ext-diff', '--find-renames']
  const diffOf = (f, u) => git([...DIFF, `-U${u}`, mergeBase, head, '--', ...(f.oldPath && f.oldPath !== f.path ? [f.oldPath, f.path] : [f.path])])
  for (const f of shown) {
    const fi = info.get(f.path)
    const full = diffOf(f, 25)
    fi.diff25 = full
    const isNew = f.status === 'A' || (f.status === 'R' && (f.added > 0 || f.deleted > 0))
    fi.points = changedPoints(full, { allLines: isNew && fi.headLines ? fi.headLines.length : 0 })
    fi.risk = riskOf(f.path)
    fi.churn = f.added + f.deleted
  }

  // RULES and REFERENCE, from the trusted tree.
  const paths = files.map((f) => f.path)
  const keys = docsFor(paths)
  const rules = keys.rules.map((k) => readDoc(git, rulesSha, k)).filter(Boolean)
  let reference = keys.reference.map((k) => readDoc(git, rulesSha, k)).filter(Boolean)

  // The changelog entry: the added changelog/ files' text.
  const entries = files.filter((f) => /^changelog\/[^/]+\.md$/.test(f.path) && f.path !== 'changelog/README.md' && f.status !== 'D')
  const changelog = capText(entries.map((f) => info.get(f.path).headText ?? '').join('\n\n').trim(), CAPS.changelog)

  // Cross-references and importers.
  const excluded = (p) => isGeneratedPath(p)
  const ids = identifiersFromDiffs(shown.map((f) => ({ path: f.path, text: info.get(f.path).diff25 })))
  let xrefs = crossReferences(git, head, ids, { isExcluded: excluded })
  const changedSet = new Set(paths)
  const modules = shown
    .filter((f) => f.status !== 'D' && /^(ui\/src|mcp\/src|cli\/src)\//.test(f.path) && !isTestPath(f.path) && /\.(ts|js|mjs|svelte)$/.test(f.path))
    .map((f) => f.path)
  let importers = findImporters(git, head, modules, changedSet, { isExcluded: excluded })

  // Per-file display state.
  const display = new Map()
  for (const f of shown) {
    const fi = info.get(f.path)
    const hunks = capHunks(fi.diff25, CAPS.hunksPerFile)
    const fullText = fi.headText !== null && f.path !== '.gitattributes' ? numberLines(fi.headText) : null
    display.set(f.path, { hunks: hunks.text, full: fullText && bytes(fullText) <= CAPS.fullFile ? fullText : null, fullSkipped: fullText && bytes(fullText) > CAPS.fullFile, hunksShown: true })
  }

  const totals = files.reduce((a, f) => ({ add: a.add + f.added, del: a.del + f.deleted }), { add: 0, del: 0 })
  const ctx = {
    head,
    mergeBase,
    rulesSha,
    trustedName,
    target,
    source,
    title: String(title ?? ''),
    body: capText(body ?? '', CAPS.body),
    authorKind: authorKind ?? 'unknown',
    changelog,
    judgePr,
    prior,
    files,
    info,
    shown,
    display,
    rules,
    reference,
    xrefs,
    importers,
    trims,
    totals,
    diffContext: 25,
    budgetTokens,
    prompt,
  }
  ctx.sections = () => renderSections(ctx)

  // The fixed part: never trimmed. Too much of it and there is no review.
  const s0 = renderSections(ctx)
  const fixedText = [prompt?.system ?? '', prompt?.review ?? '', ...rules.filter((r) => ['AGENTS.md', 'CONTRIBUTING.md'].includes(r.path)).map((r) => r.text), s0.prHeader, s0.body, s0.changelog, s0.deterministic, s0.previous, s0.files].join('\n')
  ctx.fixedTokens = estimateTokens(fixedText)
  if (!shown.length) {
    ctx.empty = true
    return ctx
  }
  if (ctx.fixedTokens > FIXED_SHARE * budgetTokens) {
    ctx.tooLarge = true
    return ctx
  }

  // Trim to the budget, in a fixed order, each step recorded.
  const total = () => estimateTokens(fullMessages(ctx, 'NONCE0000000000').join('\n'))
  const over = () => total() > budgetTokens
  const refOrder = [...REFERENCE_TRIM_ORDER, ...reference.map((r) => r.path).filter((p) => !REFERENCE_TRIM_ORDER.includes(p))]
  for (const p of refOrder) {
    if (!over()) break
    if (ctx.reference.some((r) => r.path === p)) {
      ctx.reference = ctx.reference.filter((r) => r.path !== p)
      trims.push(`reference ${p}`)
    }
  }
  if (over() && ctx.importers.length) {
    ctx.importers = []
    trims.push('importers')
  }
  if (over()) {
    const bySize = [...display.entries()].filter(([, d]) => d.full).sort((a, b) => bytes(b[1].full) - bytes(a[1].full))
    for (const [p, d] of bySize) {
      if (!over()) break
      d.full = null
      d.fullTrimmed = true
      trims.push(`full ${p}`)
    }
  }
  if (over() && ctx.xrefs.length > 10) {
    ctx.xrefs = ctx.xrefs.slice(0, 10)
    trims.push('cross-references to 10')
  }
  if (over()) {
    for (const f of shown) display.get(f.path).hunks = capHunks(diffOf(f, 10), CAPS.hunksPerFile).text
    ctx.diffContext = 10
    trims.push('diff at -U10')
  }
  if (over()) {
    const order = [...shown].sort((a, b) => info.get(a.path).risk - info.get(b.path).risk || info.get(a.path).churn - info.get(b.path).churn)
    for (const f of order) {
      if (!over()) break
      const d = display.get(f.path)
      d.hunks = null
      d.hunksShown = false
      trims.push(`hunks ${f.path}`)
    }
  }
  if (over()) ctx.tooLarge = true
  return ctx
}

/** The PR DATA sections and the doc blocks, as text. */
export function renderSections(ctx) {
  const { info } = ctx
  const prHeader = [
    '# PULL REQUEST',
    `target: ${ctx.target}`,
    `source: ${ctx.source ?? 'unknown'}`,
    `author: ${ctx.authorKind}`,
    `head: ${short(ctx.head)}`,
    `merge-base: ${short(ctx.mergeBase)}`,
    `title: ${ctx.title.replace(/\s+/g, ' ').slice(0, 300)}`,
  ].join('\n')
  const body = `## BODY (the author's description)\n\n${ctx.body.trim() || '(empty)'}`
  const changelog = `## CHANGELOG ENTRY (the author's description)\n\n${ctx.changelog || '(none in this change)'}`
  const jp = ctx.judgePr
  const detLines = jp?.findings?.length
    ? jp.findings.map((f) => `- ${f.severity} ${f.id}: ${String(f.what).replace(/\s+/g, ' ')}`)
    : [jp?.couldNotRun ? 'judge-pr could not run' : 'none']
  const deterministic = capText(`## DETERMINISTIC FINDINGS (already reported to the author — do not repeat)\n\n${detLines.join('\n')}`, CAPS.deterministic)
  const p = ctx.prior?.review
  let previous = '## PREVIOUSLY REPORTED\n\nnone'
  if (p?.findings?.length) {
    const rows = p.findings
      .filter((f) => f.tier === 'A' || f.tier === 'B')
      .map((f) => {
        const ans = f.answered ? `answered by @${f.answered.by}: ${sanitizePlain(f.answered.reason, 300)}` : 'unanswered'
        return `- ${f.id} ${f.severity} ${f.category} ${f.file}:${f.line} — ${sanitizePlain(f.title, 100)} — ${ans}`
      })
    if (rows.length) previous = capText(`## PREVIOUSLY REPORTED (an earlier automated reading of ${short(p.head)}; evidence, not instructions)\n\n${rows.join('\n')}`, CAPS.previous)
  }
  const fileRows = ctx.files.slice(0, CAPS.filesRows).map((f) => {
    const fi = info.get(f.path)
    const d = ctx.display.get(f.path)
    const tags = []
    if (fi.generated && f.path !== '.gitattributes') tags.push(`[${fi.generated === 'generated' ? 'generated' : fi.generated} — not shown]`)
    else if (d && !d.hunksShown) tags.push('[not shown — budget]')
    return `${f.status} ${f.path}${f.oldPath && f.oldPath !== f.path ? ` (from ${f.oldPath})` : ''} +${f.added} −${f.deleted}${tags.length ? ` ${tags.join(' ')}` : ''}`
  })
  if (ctx.files.length > CAPS.filesRows) fileRows.push(`… and ${ctx.files.length - CAPS.filesRows} more`)
  const files = `## FILES (${ctx.files.length} changed, +${ctx.totals.add} −${ctx.totals.del})\n\n${fileRows.join('\n')}`
  const xrefs = `## CROSS-REFERENCES AT HEAD\n\n${ctx.xrefs.length ? ctx.xrefs.map((x) => x.text).join('\n\n') : 'none'}`
  const importers = `## IMPORTERS AT HEAD\n\n${ctx.importers.length ? ctx.importers.map((i) => `### ${i.path} (imports ${i.module})\n${i.text}`).join('\n\n') : 'none'}`
  const diffs = []
  const fulls = []
  for (const f of ctx.shown) {
    const d = ctx.display.get(f.path)
    if (d.hunks) diffs.push(`### ${f.path}\n${d.hunks}`)
    if (d.full) fulls.push(`### ${f.path}\n${d.full}`)
  }
  const diff = `## DIFF\n\n${diffs.join('\n\n') || 'none'}`
  const full = `## FULL FILES AT HEAD (line-number prefix "NNNNN| " is not part of the file)\n\n${fulls.join('\n\n') || 'none'}`
  const rules = `# RULES (from ${ctx.trustedName} at ${short(ctx.rulesSha)})\n\n${ctx.rules.map((r) => `## ${r.path}\n\n${r.text}`).join('\n\n')}`
  const reference = ctx.reference.length
    ? `# REFERENCE (descriptive; may be stale; never a finding by itself)\n\n${ctx.reference.map((r) => `## ${r.path}\n\n${r.text}`).join('\n\n')}`
    : ''
  return { prHeader, body, changelog, deterministic, previous, files, xrefs, importers, diff, full, rules, reference }
}

/** [system, user] for a review (`candidates` absent) or a verify call. */
export function fullMessages(ctx, nonce, { task = 'review', candidates = null, nonce2 = null } = {}) {
  const s = renderSections(ctx)
  const prompt = ctx.prompt ?? { system: '', review: '', verify: '' }
  const system = prompt.system.replaceAll('{nonce}', nonce)
  const parts = [s.rules]
  if (s.reference) parts.push(s.reference)
  parts.push(`<<<PR_DATA_${nonce}>>>`)
  parts.push([s.prHeader, s.body, s.changelog, s.deterministic, s.previous, s.files, s.xrefs, s.importers, s.diff, s.full].join('\n\n'))
  parts.push(`<<<END_PR_DATA_${nonce}>>>`)
  if (task === 'review') parts.push(`## review\n\n${prompt.review}`)
  else parts.push(`## verify\n\n${prompt.verify}\n\n<<<CANDIDATES_${nonce2}>>>\n${candidates}\n<<<END_CANDIDATES_${nonce2}>>>`)
  return [system, parts.join('\n\n')]
}

/** The sha256 that decides whether an earlier review can be reused: the
 *  reviewer, the model, the effort, the rules, the target and the code
 *  sections — never the title, body, changelog or earlier findings. */
export function contextHash(ctx, { version, model, effort = 'high' }) {
  const s = renderSections(ctx)
  return sha256([version, model, effort, ctx.rulesSha, ctx.target, s.files, s.xrefs, s.importers, s.diff, s.full].join('\0'))
}

export function makeNonce(avoid = '') {
  for (;;) {
    const n = randomBytes(8).toString('hex')
    if (!avoid.includes(n)) return n
  }
}

/** The context plan: sizes, trims and the estimate. */
export function contextPlan(ctx) {
  const s = renderSections(ctx)
  const sections = {}
  for (const k of ['rules', 'reference', 'prHeader', 'body', 'changelog', 'deterministic', 'previous', 'files', 'xrefs', 'importers', 'diff', 'full']) sections[k] = bytes(s[k])
  const msgs = fullMessages(ctx, '0000000000000000')
  const inputTokensEst = estimateTokens(msgs.join('\n'))
  return {
    files: ctx.files.length,
    filesShown: ctx.shown.filter((f) => ctx.display.get(f.path)?.hunksShown).length,
    filesNotShown: ctx.shown.filter((f) => !ctx.display.get(f.path)?.hunksShown).map((f) => f.path),
    rules: ctx.rules.map((r) => r.path),
    reference: ctx.reference.map((r) => r.path),
    trims: [...ctx.trims],
    partial: ctx.shown.some((f) => !ctx.display.get(f.path)?.hunksShown),
    diffContext: ctx.diffContext,
    inputTokensEst,
    sections,
  }
}

// ── filters ─────────────────────────────────────────────────────────────────

/** A quote as the anchor compares it: no line-number prefixes, no diff
 *  markers (when every line has one), whitespace collapsed, no blank lines. */
export function normalizeQuote(quote) {
  let lines = String(quote ?? '').replace(/\r/g, '').split('\n').map((l) => l.replace(/^\s*\d+\| ?/, ''))
  const nonEmpty = lines.filter((l) => l.length)
  if (nonEmpty.length && nonEmpty.every((l) => /^[+ ]/.test(l))) lines = lines.map((l) => l.slice(1))
  return lines.map(norm).filter(Boolean)
}

/** Where a normalised quote sits in the head's lines: `{line, endLine,
 *  anchorText, reanchored}` or `{drop}`. */
export function anchorQuote(headLines, quoteLines, statedLine) {
  const nonEmpty = []
  headLines.forEach((l, i) => {
    const n = norm(l)
    if (n) nonEmpty.push({ n, line: i + 1 })
  })
  const matches = []
  for (let i = 0; i + quoteLines.length <= nonEmpty.length; i++) {
    let ok = true
    for (let j = 0; j < quoteLines.length; j++) {
      if (nonEmpty[i + j].n !== quoteLines[j]) {
        ok = false
        break
      }
    }
    if (ok) matches.push({ line: nonEmpty[i].line, endLine: nonEmpty[i + quoteLines.length - 1].line })
  }
  if (!matches.length) return { drop: 'quote-not-in-head' }
  const near = matches.filter((m) => Math.abs(m.line - statedLine) <= ANCHOR_WINDOW).sort((a, b) => Math.abs(a.line - statedLine) - Math.abs(b.line - statedLine))
  if (near.length) return { ...near[0], anchorText: quoteLines, reanchored: false }
  if (matches.length === 1) return { ...matches[0], anchorText: quoteLines, reanchored: true }
  return { drop: 'quote-ambiguous' }
}

const DETERMINISTIC_DUP = /changelog entry|commit subject|target(s|ed)? (main|rc)|pull request body|PR body|generated (doc|reference)/i

function checkFindingShape(f) {
  if (!f || typeof f !== 'object') return false
  if (!['blocker', 'major', 'minor'].includes(f.severity)) return false
  if (!CATEGORIES.includes(f.category)) return false
  if (!RULE_IDS.includes(f.rule_id)) return false
  if (!['high', 'medium', 'low'].includes(f.confidence)) return false
  if (!Number.isInteger(f.line) || f.line < 1) return false
  for (const k of ['file', 'quote', 'title', 'failure', 'fix', 'rule']) if (typeof f[k] !== 'string') return false
  return f.file.length > 0 && f.title.trim().length > 0
}

/**
 * The model's findings, filtered in code. `ctx.info` is the changed-file map
 * (path → {exists, generated, headLines, points}), `ctx.rulesSent` the RULES
 * doc paths, `ctx.filePaths` the FILES list. Every drop is in `dropped`.
 */
export function filterFindings(raw, ctx) {
  const kept = []
  const dropped = []
  const drop = (reason, finding, evidence) => dropped.push({ reason, ...(evidence ? { evidence } : {}), finding })
  for (const f of Array.isArray(raw) ? raw : []) {
    if (!checkFindingShape(f)) {
      drop('schema', f)
      continue
    }
    const fi = ctx.info.get(f.file)
    if (!fi || !fi.exists) {
      drop('file-not-changed', f)
      continue
    }
    if (fi.generated || fi.binary || !fi.headLines) {
      drop('file-generated', f)
      continue
    }
    const q = normalizeQuote(f.quote)
    if (q.join('').replace(/\s/g, '').length < MIN_QUOTE_CHARS) {
      drop('quote-too-short', f)
      continue
    }
    const a = anchorQuote(fi.headLines, q, f.line)
    if (a.drop) {
      drop(a.drop, f)
      continue
    }
    const points = fi.points ?? new Set()
    let dist = Infinity
    let touches = false
    for (const p of points) {
      if (p >= a.line && p <= a.endLine) {
        touches = true
        dist = 0
        break
      }
      dist = Math.min(dist, p < a.line ? a.line - p : p - a.endLine)
    }
    if (!touches && dist > HUNK_WINDOW) {
      drop('outside-diff', f)
      continue
    }
    if (f.confidence === 'low') {
      drop('low-confidence', f)
      continue
    }
    let severity = f.severity
    if (CAPPED_AT_MINOR.includes(f.category)) severity = 'minor'
    else if (CAPPED_AT_MAJOR.includes(f.category) && severity === 'blocker') severity = 'major'
    else if (f.category === 'repo-rule' && f.rule_id === 'none' && severity === 'blocker') severity = 'major'
    if (f.category === 'repo-rule') {
      const rule = String(f.rule)
      const cited = (ctx.rulesSent ?? []).some((d) => rule.includes(d.split('#')[0])) || (ctx.filePaths ?? []).some((p) => rule.includes(p))
      if (!cited) {
        drop('rule-not-cited', f)
        continue
      }
      if (!/^(scripts|\.github)\//.test(f.file) && DETERMINISTIC_DUP.test(`${f.title} ${f.failure}`)) {
        drop('deterministic-duplicate', f)
        continue
      }
    }
    kept.push({
      id: findingId(f.file, fi.headLines, a.line),
      severity,
      severityRaw: f.severity,
      category: f.category,
      ruleId: f.rule_id,
      confidence: f.confidence,
      file: f.file,
      line: a.line,
      endLine: a.endLine,
      anchorText: a.anchorText,
      quote: fi.headLines.slice(a.line - 1, a.endLine).join('\n'),
      title: f.title,
      failure: f.failure,
      fix: f.fix,
      rule: f.rule,
      reanchored: a.reanchored,
      touchesChange: touches,
    })
  }
  // Dedupe: by id, then by place; the higher severity, then confidence, stays.
  const better = (a, b) => SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity] || CONFIDENCE_RANK[a.confidence] - CONFIDENCE_RANK[b.confidence]
  const out = []
  for (const f of kept) {
    const twin = out.findIndex((o) => o.id === f.id || (o.file === f.file && Math.abs(o.line - f.line) <= 3))
    if (twin === -1) out.push(f)
    else if (better(f, out[twin]) < 0) {
      drop('duplicate', out[twin])
      out[twin] = f
    } else drop('duplicate', f)
  }
  out.sort(better)
  for (const f of out.slice(MAX_FINDINGS)) drop('over-cap', f)
  return { kept: out.slice(0, MAX_FINDINGS), dropped }
}

// ── answers, verify, tiers, the gate ────────────────────────────────────────

/** Answers by id, or carried from an answered prior finding on the same
 *  file whose anchored lines share one of 20+ non-space characters. */
export function applyAnswers(findings, answers, priorFindings = []) {
  for (const f of findings) {
    const direct = answers.get(f.id)
    if (direct) {
      f.answered = { ...direct, carriedFrom: null }
      continue
    }
    const pa = priorFindings.find((p) => {
      if (p.file !== f.file) return false
      const ans = p.answered ?? answers.get(p.id)
      if (!ans) return false
      const shared = (p.anchorText ?? []).filter((l) => l.replace(/\s/g, '').length >= MIN_QUOTE_CHARS)
      return shared.some((l) => (f.anchorText ?? []).includes(l))
    })
    if (pa) {
      const ans = pa.answered ?? answers.get(pa.id)
      f.answered = { by: ans.by, reason: ans.reason, url: ans.url ?? '', carriedFrom: pa.id }
    } else f.answered = null
  }
  return findings
}

const RULE_PATHS = {
  'gateway-bypass': (p) => /^(api\/crates\/|ui\/src\/server\/|mcp\/src\/)/.test(p),
  'secret-in-config': (p) => /\.(ya?ml|json|toml|ini|conf|env)$|(^|\/)\.env/.test(p) && !/^(\.github|docs)\//.test(p) && !/fixture/i.test(p),
  'forced-done': (p) => /^(api\/crates\/|ui\/src\/server\/|mcp\/src\/|cli\/src\/)/.test(p),
}

/** The removed-call identifiers whose last non-test use the diff removed. */
function deadCallIds(xrefs) {
  return (xrefs ?? []).filter((x) => x.kind === 'removed-call' && x.nonTestUses === 0).map((x) => x.id)
}

/** The gate conditions a finding fails, by name. `phase: 'pre'` leaves out
 *  the ones only verification can settle (tier and the two confirmations). */
export function gateBlockers(f, { lane, priorGateHeads = 0, deadIds = [], phase = 'final' }) {
  const out = []
  if (phase === 'final' && f.tier !== 'A') out.push('tier-a')
  if (f.severity !== 'blocker') out.push('blocker')
  if (GATE_NEEDS_HIGH && f.confidence !== 'high') out.push('confidence-high')
  if (phase === 'final' && !(f.verified === 'confirmed' && (f.confirmedBy ?? 0) >= 2)) out.push('two-confirmations')
  if (f.reanchored) out.push('reanchored')
  if (!f.touchesChange) out.push('touches-change')
  if (!['feature', 'hotfix'].includes(lane)) out.push('lane')
  if (priorGateHeads >= 2) out.push('gate-heads')
  if (!GATING_CATEGORIES.includes(f.category)) out.push('category')
  if (f.category === 'repo-rule') {
    const fits = f.ruleId !== 'none' && RULE_PATHS[f.ruleId]?.(f.file) && !isTestPath(f.file) && !/^(scripts|\.github)\//.test(f.file)
    if (!fits) out.push('rule-path')
  }
  if (f.category === 'dead-wiring') {
    const text = `${f.title} ${f.failure} ${f.quote}`
    if (!deadIds.some((id) => new RegExp(`\\b${id}\\b`).test(text))) out.push('removed-call-fact')
    else f.removedCallFact = deadIds.find((id) => new RegExp(`\\b${id}\\b`).test(text))
  }
  if (f.answered) out.push('answered')
  return out
}

/** The blind rendering of the candidates: index, file, line, a short label
 *  and the head's own lines. No severity, category, failure, fix or confidence. */
export function renderCandidates(cands) {
  return cands
    .map((f, i) => [`[${i}] file: ${f.file}`, `line: ${f.line}`, `label: ${String(f.title).replace(/\s+/g, ' ').slice(0, 100)}`, 'quoted lines:', f.quote].join('\n'))
    .join('\n\n')
}

/** One verdict per finding from the samples that ran. */
export function combineVerdicts(samples) {
  const eff = samples.map((s) => (s.verdict === 'confirmed' && (s.input ?? '').trim().length >= 10 && (s.trace ?? '').trim().length >= 20 ? 'confirmed' : s.verdict === 'confirmed' ? 'unsure' : s.verdict))
  if (!eff.length) return { verified: 'skipped', confirmedBy: 0 }
  const c = eff.filter((v) => v === 'confirmed').length
  const r = eff.filter((v) => v === 'refuted').length
  if (c === eff.length) return { verified: 'confirmed', confirmedBy: c }
  if (c === 0 && r > 0) return { verified: 'refuted', confirmedBy: 0 }
  if (c > 0) return { verified: 'split', confirmedBy: c }
  return { verified: 'unsure', confirmedBy: 0 }
}

/** Tiers: A (confirmed, major or above, a listed category, unanswered; the
 *  top three), B (every other blocker or major), C (minors). */
export function assignTiers(findings) {
  const listed = findings
    .filter((f) => f.severity !== 'minor' && f.verified === 'confirmed' && LISTED_CATEGORIES.includes(f.category) && !f.answered)
    .sort((a, b) => SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity] || Number(b.touchesChange) - Number(a.touchesChange) || CONFIDENCE_RANK[a.confidence] - CONFIDENCE_RANK[b.confidence])
  const a = new Set(listed.slice(0, MAX_LISTED))
  for (const f of findings) f.tier = a.has(f) ? 'A' : f.severity === 'minor' ? 'C' : 'B'
  return findings
}

/** Marks: `gate` under JUDGE_REVIEW_GATE=blockers, `wouldGate` otherwise. */
export function markGate(findings, { mode, lane, priorGateHeads, deadIds }) {
  for (const f of findings) {
    f.gateBlockedBy = gateBlockers(f, { lane, priorGateHeads, deadIds })
    const pass = f.gateBlockedBy.length === 0
    f.gate = mode === 'blockers' && pass
    f.wouldGate = mode !== 'blockers' && pass
  }
  return findings
}

// ── sanitising and rendering ────────────────────────────────────────────────

// eslint-disable-next-line no-control-regex
const C0C1 = /[\u0000-\u001f\u007f-\u009f]/g

/** Model- or PR-derived text for a comment, a summary or a table cell. */
export function sanitizeText(s, cap = 400) {
  let t = String(s ?? '').replace(C0C1, ' ').replace(BIDI_ZW_RE, '')
  if (t.length > cap) t = `${t.slice(0, cap - 1)}…`
  t = t.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  t = t.replace(/[\\`*_[\]()!|~]/g, (c) => `\\${c}`)
  t = t.replace(/^(\s*)#/, '$1\\#')
  t = t.replace(/@/g, '@​').replace(/#(\d)/g, '#​$1').replace(/\bGH-(\d)/gi, (m, d) => `${m.slice(0, 2)}​-${d}`)
  t = t.replace(/:\/\//g, ':​//')
  return t
}

/** Plain text for the model's PREVIOUSLY REPORTED section: one line, capped. */
function sanitizePlain(s, cap) {
  const t = String(s ?? '').replace(C0C1, ' ').replace(BIDI_ZW_RE, '').replace(/\s+/g, ' ').trim()
  return t.length > cap ? `${t.slice(0, cap - 1)}…` : t
}

/** A path as inline code, or a placeholder when it holds an unprintable character. */
export function renderPath(p) {
  const s = String(p ?? '')
  const bad = (s.match(C0C1) ?? []).length + (s.match(BIDI_ZW_RE) ?? []).length
  if (bad) return `&lt;path with ${bad} unprintable characters&gt;`
  const runs = s.match(/`+/g) ?? []
  const fence = '`'.repeat(Math.max(0, ...runs.map((r) => r.length)) + 1)
  const pad = s.startsWith('`') || s.endsWith('`') ? ' ' : ''
  return `${fence}${pad}${s.replace(/\|/g, '\\|')}${pad}${fence}`
}

const LANGS = { rs: 'rust', ts: 'ts', svelte: 'svelte', mjs: 'js', js: 'js', yml: 'yaml', yaml: 'yaml', sql: 'sql', md: 'markdown', toml: 'toml', sh: 'sh' }

/** A fenced block of the head's quoted lines. */
export function renderQuote(quote, file) {
  let lines = String(quote ?? '').replace(/\r/g, '').split('\n').slice(0, 5).map((l) => l.replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, '').replace(BIDI_ZW_RE, ''))
  let text = lines.join('\n')
  if (text.length > 500) text = `${text.slice(0, 499)}…`
  lines = text.split('\n').map((l) => (/^\s*<!--/.test(l) ? l.replace('<', '<​') : l))
  text = lines.join('\n')
  const runs = text.match(/`+/g) ?? []
  const fence = '`'.repeat(Math.max(3, ...runs.map((r) => r.length + 1)))
  const ext = /\.([a-z]+)$/.exec(String(file))?.[1] ?? ''
  return `${fence}${LANGS[ext] ?? ''}\n${text}\n${fence}`
}

function blobLink(repo, head, path) {
  return `https://github.com/${repo}/blob/${head}/${String(path).split('/').map(encodeURIComponent).join('/')}`
}

const fmtCost = (c) => (typeof c === 'number' && Number.isFinite(c) ? c.toFixed(4) : 'na')
const fmtMs = (ms) => `${Math.floor(ms / 60000)}m${String(Math.floor((ms % 60000) / 1000)).padStart(2, '0')}s`

function marker(r) {
  return `<!-- judge-review v2 head=${short(r.head) || 'na'} base=${short(r.base) || 'na'} target=${slug(r.target)} model=${slug(r.model?.served ?? r.model?.requested)} provider=${slug(r.model?.provider)} cost=${fmtCost(r.usage?.cost)} gate=${slug(r.gateMode)} hash=${short(r.contextHash) || 'na'} -->`
}

/** The PR comment: Tier A in full, Tier B collapsed; under 20,000 characters. */
export function renderComment(r, { repo = 'outcrop-labs/talaria' } = {}) {
  const head12 = short(r.head)
  if (r.status !== 'reviewed') {
    return `${marker(r)}\n## judge (review) — not reviewed at \`${head12 || 'unknown'}\`: ${sanitizeText(r.reasonText || r.reason, 300)}\n`
  }
  const a = r.findings.filter((f) => f.tier === 'A')
  const b = r.findings.filter((f) => f.tier === 'B')
  const target = slug(r.target)
  const ctxInfo = r.context ?? {}
  const meta = [
    `${ctxInfo.filesShown ?? 0}/${ctxInfo.files ?? 0} files read (+${r.totals?.add ?? 0} −${r.totals?.del ?? 0})`,
    `${slug(r.model?.served ?? r.model?.requested)} via ${slug(r.model?.provider)}`,
    `$${fmtCost(r.usage?.cost)}`,
    fmtMs(r.usage?.ms ?? 0),
  ]
  const tail = `${r.reused ? `, reused from run ${r.reused.fromRun}` : ''}${ctxInfo.partial ? `, partial: ${ctxInfo.filesNotShown?.length ?? 0} files not shown` : ''}`
  const out = [marker(r), `## judge (review) — \`${head12}\` vs \`${target}\``]
  out.push(`**${a.length ? `${a.length} to read` : 'Nothing to read'}** — ${meta.join(' · ')}${tail}`)
  if (!a.length) out.push('', `**Nothing to read at ${head12}.** Earlier findings were fixed or not raised again.`)
  if (r.summary) out.push('', `> ${sanitizeText(r.summary, 400)}`)
  const block = (f, i, withFix) => {
    const conf = (f.confirmedBy ?? 0) >= 2 ? 'confirmed by two blind second readings' : 'confirmed by a blind second reading'
    const holds = f.gate ? " · **holds an agent's done until fixed or answered**" : ''
    const lines = [
      `### ${i + 1} · ${f.severity} · ${f.category} — ${sanitizeText(f.title, 100)}`,
      `\`${f.id}\` · [${renderPath(`${f.file}:${f.line}`)}](${blobLink(repo, r.head, f.file)}#L${f.line}) · ${f.verified === 'confirmed' ? conf : `not confirmed (${f.verified})`}${holds}`,
      renderQuote(f.quote, f.file),
      `**What breaks:** ${sanitizeText(f.failure, 600)}`,
    ]
    if (withFix) lines.push(`**Fix:** ${sanitizeText(f.fix, 400)}`)
    if (f.rule && f.rule !== 'none') lines.push(`**Rule:** ${sanitizeText(f.rule, 200)}`)
    if (f.answered) lines.push(`**Answered** by @​${sanitizeText(f.answered.by, 60)}: “${sanitizeText(f.answered.reason, 300)}”`)
    return lines.join('\n')
  }
  a.forEach((f, i) => out.push('', block(f, i, true)))
  const footer = [
    '',
    '---',
    `<sub>A model's reading of this change, not an approval. Each finding above was confirmed by a second reading that saw only the quoted lines. ${
      r.gateMode === 'blockers'
        ? 'A finding marked "holds" holds an agent\'s done until it is fixed or answered with `judge-review answer <id>: <why the code is right>` (40+ characters); it never holds a person\'s merge.'
        : 'Nothing here holds a merge. A finding you think is wrong needs no reply.'
    } Minor notes, the verification gap and usage are in the job summary.</sub>`,
  ]
  let bBlocks = b.map((f, i) => block(f, i, false))
  const assemble = () => {
    const parts = [...out]
    if (bBlocks.length) parts.push('', `<details><summary>${bBlocks.length} more, not confirmed by a second reading (no action needed)</summary>`, '', ...bBlocks.flatMap((x) => [x, '']), '</details>')
    return [...parts, ...footer].join('\n')
  }
  let text = assemble()
  while (text.length > 20_000 && bBlocks.length) {
    bBlocks = bBlocks.slice(0, -1)
    text = assemble()
  }
  return text.length > 20_000 ? `${text.slice(0, 19_990)}\n</sub>` : `${text}\n`
}

/** The job summary: everything, Tier C and the drops included. */
export function renderSummary(r) {
  const out = [`## judge (review) — ${r.status}${r.reason ? ` (${r.reason})` : ''} at \`${short(r.head)}\``, '', `\`${verdictMessage(verdictOf(r), '')}\``]
  if (r.reasonText) out.push('', sanitizeText(r.reasonText, 600))
  if (r.summary) out.push('', `> ${sanitizeText(r.summary, 400)}`)
  const table = (rows, head) => (rows.length ? [`| ${head.join(' | ')} |`, `| ${head.map(() => '---').join(' | ')} |`, ...rows.map((r2) => `| ${r2.join(' | ')} |`)].join('\n') : '_none_')
  const tierRows = (t) => (r.findings ?? []).filter((f) => f.tier === t).map((f) => [`\`${f.id}\``, f.severity, f.category, renderPath(`${f.file}:${f.line}`), f.verified, sanitizeText(f.title, 100), sanitizeText(f.failure, 300)])
  out.push('', '### Tier A — listed', table(tierRows('A'), ['id', 'severity', 'category', 'place', 'verified', 'title', 'failure']))
  out.push('', '### Tier B — not confirmed or over the cap', table(tierRows('B'), ['id', 'severity', 'category', 'place', 'verified', 'title', 'failure']))
  out.push('', '### Tier C — minor', table(tierRows('C'), ['id', 'severity', 'category', 'place', 'verified', 'title', 'failure']))
  const gated = (r.findings ?? []).filter((f) => f.tier && f.severity === 'blocker')
  out.push('', '### Gate', table(gated.map((f) => [`\`${f.id}\``, f.gate ? 'gate' : f.wouldGate ? 'would-gate' : 'no', (f.gateBlockedBy ?? []).join(', ') || '—']), ['id', 'mark', 'blocked by']))
  if (r.verificationGap) out.push('', `**Verification gap:** ${sanitizeText(r.verificationGap, 300)}`)
  if (r.couldNotJudge?.length) out.push('', '**Could not judge:**', ...r.couldNotJudge.slice(0, 3).map((c) => `- ${sanitizeText(c, 200)}`))
  out.push('', '### Dropped', table((r.dropped ?? []).map((d) => [d.reason, renderPath(`${d.finding?.file ?? '?'}:${d.finding?.line ?? '?'}`), sanitizeText(d.finding?.title ?? '', 100)]), ['reason', 'place', 'title']))
  const c = r.context ?? {}
  out.push('', '### Context', table([
    ['rules', (c.rules ?? []).join(', ') || '—'],
    ['reference', (c.reference ?? []).join(', ') || '—'],
    ['files', `${c.filesShown ?? 0} shown of ${c.files ?? 0}${c.partial ? ' (partial)' : ''}`],
    ['diff context', `-U${c.diffContext ?? 25}`],
    ['trims', (c.trims ?? []).join(', ') || '—'],
    ['input estimate', `${c.inputTokensEst ?? 0} tokens`],
    ['sections (bytes)', Object.entries(c.sections ?? {}).map(([k, v]) => `${k} ${v}`).join(', ')],
    ['context hash', `\`${short(r.contextHash)}\``],
    ['reused', r.reused ? `run ${r.reused.fromRun}` : 'no'],
  ], ['what', 'value']))
  const calls = r.usage?.calls ?? []
  out.push('', '### Usage', table(calls.map((u) => [u.purpose, u.attempt, slug(u.provider), u.effort, u.promptTokens ?? '—', u.cachedTokens ?? '—', u.completionTokens ?? '—', u.reasoningTokens ?? '—', u.tokensPerSec ?? '—', fmtCost(u.cost), u.ms, slug(u.finish ?? u.errorType ?? '—')]), ['purpose', 'attempt', 'provider', 'effort', 'prompt', 'cached', 'completion', 'reasoning', 'tok/s', 'cost', 'ms', 'finish']))
  out.push('', `Total cost: $${fmtCost(r.usage?.cost)}${r.usage?.ceilingHit ? ' (ceiling reached)' : ''}`)
  return `${out.join('\n')}\n`
}

/** The verdict's values from a result. */
export function verdictOf(r) {
  const fs = r.findings ?? []
  return {
    status: r.status,
    head: short(r.head) || undefined,
    base: short(r.base) || 'na',
    target: slug(r.target),
    listed: fs.filter((f) => f.tier === 'A').length,
    more: fs.filter((f) => f.tier === 'B').length,
    gating: fs.filter((f) => f.gate).length,
    would: fs.filter((f) => f.wouldGate).length,
    gate: GATE_MODES.includes(r.gateMode) ? r.gateMode : 'na',
    cost: fmtCost(r.usage?.cost),
    model: slug(r.model?.served ?? r.model?.requested),
    provider: slug(r.model?.provider),
    reason: slug(r.reason ?? 'none'),
    reused: r.reused ? String(r.reused.fromRun).replace(/\D/g, '') || 'none' : 'none',
    partial: r.context?.partial ? 'yes' : 'no',
  }
}

function humanVerdict(r) {
  if (r.status === 'skipped') return `skipped: ${r.reasonText || r.reason}. Not a finding.`
  if (r.status !== 'reviewed') return `not reviewed: ${r.reasonText || r.reason}. Not a finding.`
  const a = r.findings.filter((f) => f.tier === 'A').length
  return a ? `${a} confirmed finding(s) to read; the PR comment has each one.` : 'nothing to read.'
}

/** The workflow commands under --github: the verdict, then one warning per Tier A finding. */
export function annotations(r) {
  const out = [`::notice title=${escapeProperty(VERDICT_TITLE)}::${escapeData(verdictMessage(verdictOf(r), humanVerdict(r)))}`]
  for (const f of (r.findings ?? []).filter((x) => x.tier === 'A').slice(0, MAX_LISTED)) {
    const title = findingTitle({ id: f.id, severity: f.severity, category: f.category, mark: f.gate ? 'gate' : f.wouldGate ? 'would-gate' : null })
    let msg = `${safeTerminal(String(f.title).replace(/\s+/g, ' '), 200)} — ${safeTerminal(String(f.failure).replace(/\s+/g, ' '), 700)} (the PR comment has the fix)`
    if (msg.length > 1000) msg = `${msg.slice(0, 999)}…`
    out.push(`::warning file=${escapeProperty(f.file)},line=${f.line},endLine=${f.endLine},title=${escapeProperty(title)}::${escapeData(msg)}`)
  }
  return out
}

/** The local markdown review: judge-pr first, then the findings. */
export function renderLocal(r) {
  const out = []
  if (r.judgePr?.report) out.push(r.judgePr.report.trim(), '')
  out.push(`## judge (review) — ${r.status}${r.reason ? ` (${r.reason})` : ''} at ${short(r.head)} vs ${r.target}`)
  if (r.reasonText) out.push('', safeTerminal(r.reasonText, 600))
  if (r.summary) out.push('', `> ${safeTerminal(r.summary, 400)}`)
  for (const t of ['A', 'B', 'C']) {
    const fs = (r.findings ?? []).filter((f) => f.tier === t)
    if (!fs.length) continue
    out.push('', `### ${t === 'A' ? 'To read (confirmed)' : t === 'B' ? 'Not confirmed — no action needed' : 'Minor'}`)
    for (const f of fs) {
      out.push('', `- ${f.id} ${f.severity} ${f.category} ${f.file}:${f.line} — ${safeTerminal(f.title, 200)}${f.answered ? ' (answered)' : ''}`)
      if (t === 'A') {
        out.push(`  what breaks: ${safeTerminal(f.failure, 600)}`)
        out.push(`  fix: ${safeTerminal(f.fix, 400)}`)
      }
    }
  }
  if (r.verificationGap) out.push('', `verification gap: ${safeTerminal(r.verificationGap, 300)}`)
  return `${out.join('\n')}\n`
}

// ── prior reviews ───────────────────────────────────────────────────────────

/** `--prior <dir>`: index.json and the downloaded review.json of each run. */
export function loadPrior(dir) {
  const empty = { latest: null, samePush: [] }
  if (!dir || !existsSync(join(dir, 'index.json'))) return empty
  const read = (run) => {
    try {
      const r = JSON.parse(readFileSync(join(dir, String(run), 'review.json'), 'utf8'))
      return r?.version === 2 ? { run: String(run), review: r } : null
    } catch {
      return null
    }
  }
  try {
    const idx = JSON.parse(readFileSync(join(dir, 'index.json'), 'utf8'))
    return {
      latest: idx.latestPr?.run ? read(idx.latestPr.run) : null,
      samePush: (idx.samePush ?? []).map(read).filter(Boolean),
    }
  } catch {
    return empty
  }
}

// ── the review ──────────────────────────────────────────────────────────────

const REASON_TEXT = {
  'no-key': `no ${KEY_ENV} in this job — the owner adds it as a repository secret`,
  'key-rejected': 'OpenRouter refused the key',
  'out-of-credit': "the key's spend limit is reached",
  'no-endpoint': 'no OpenRouter endpoint serves the model under these provider filters',
  'request-rejected': 'OpenRouter rejected the request',
  'provider-error': 'the provider failed',
  timeout: 'the review ran out of time',
  truncated: 'the output was cut off',
  refused: 'the model refused',
  'invalid-output': 'the output did not match the schema',
  'cost-ceiling': 'the per-run cost ceiling was reached',
  'too-large': 'the change is too large to read whole',
  empty: 'nothing reviewable changed (generated or binary files only)',
  lane: 'a promotion, snapshot, back-flow, sync or roll — each change in it was reviewed in its own pull request',
  crash: 'the reviewer crashed',
  offline: 'the network is refused (JUDGE_REVIEW_OFFLINE)',
  'no-reviewer-on-trusted': 'the trusted branch has no reviewer yet; this was a dry run of the head\'s copy, with no key',
  'dry-run': 'a dry run: no model call',
  setup: 'the range could not be read',
}

function baseResult(o) {
  return {
    version: 2,
    status: null,
    reason: null,
    reasonText: '',
    event: o.event ?? null,
    head: null,
    base: null,
    baseRef: o.base ?? null,
    rulesFrom: o.rulesFrom ?? null,
    trusted: null,
    target: o.target ?? null,
    source: o.source ?? null,
    pr: o.pr ?? null,
    lane: null,
    gateMode: o.gateMode ?? 'report',
    reviewerVersion: o.reviewerVersion ?? null,
    contextHash: null,
    reused: null,
    model: { requested: o.model ?? DEFAULT_MODEL, served: null, provider: null },
    context: null,
    totals: null,
    summary: '',
    verificationGap: '',
    couldNotJudge: [],
    findings: [],
    dropped: [],
    prior: null,
    gateHeads: 0,
    gateHeadList: [],
    judgePr: null,
    usage: { calls: [], cost: 0, ms: 0, ceilingHit: false },
  }
}

function runJudgePr({ cwd, scriptDir, base, head, target, source, body, tmp }) {
  const args = [join(scriptDir, 'judge-pr.mjs'), '--base', base, '--head', head, '--json']
  if (target) args.push('--target', target)
  if (source) args.push('--source', source)
  if (body !== null && body !== undefined) {
    const f = join(tmp, 'judge-pr-body.md')
    writeFileSync(f, body)
    args.push('--pr-body', f)
  }
  const r = spawnSync(process.execPath, args, { cwd, env: childEnv(), encoding: 'utf8', timeout: TIMEOUTS.judgePr, maxBuffer: 64 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] })
  if (r.status === 0 || r.status === 2) {
    try {
      const j = JSON.parse(r.stdout)
      const plain = spawnSync(process.execPath, args.filter((a) => a !== '--json'), { cwd, env: childEnv(), encoding: 'utf8', timeout: TIMEOUTS.judgePr, maxBuffer: 64 * 1024 * 1024 })
      return { exit: r.status, counts: j.counts, findings: j.findings ?? [], ids: (j.findings ?? []).map((f) => f.id), report: plain.stdout ?? '' }
    } catch {
      // fall through
    }
  }
  return { exit: r.status ?? null, couldNotRun: true, counts: null, findings: [], ids: [], report: '', error: safeTerminal(String(r.stderr ?? r.error?.message ?? ''), 400) }
}

/**
 * One review, end to end. Never throws; never exits. `env` supplies the key
 * (already taken) and the injectables (fetch, sleep, now).
 */
export async function review(o, env = {}) {
  const now = env.now ?? Date.now
  const t0 = now()
  const git = env.git ?? makeGit(o.cwd ?? process.cwd())
  const r = baseResult(o)
  const finish = (status, reason, extra = {}) => {
    Object.assign(r, { status, reason, reasonText: extra.reasonText ?? REASON_TEXT[reason] ?? reason ?? '' }, extra)
    r.usage.ms = now() - t0
    return r
  }
  try {
    let headSha
    let mergeBase
    let baseRef = o.base ?? 'auto'
    let target = o.target ?? 'auto'
    try {
      headSha = git(['rev-parse', '--verify', `${o.head ?? 'HEAD'}^{commit}`])
      if (baseRef === 'auto') {
        const wrapped = (args) => git(headWrapped(args, headSha))
        const picked = pickBase(wrapped)
        if (!picked) return finish('not-reviewed', 'setup', { reasonText: 'neither origin/testing nor origin/rc is in this clone — fetch them' })
        baseRef = picked.ref
        mergeBase = picked.sha
        if (target === 'auto') target = picked.ref.replace(/^origin\//, '')
      } else {
        mergeBase = git(['merge-base', baseRef, headSha])
        if (target === 'auto') target = baseRef.replace(/^origin\//, '')
      }
    } catch (e) {
      return finish('not-reviewed', 'setup', { reasonText: `the range could not be read: ${safeTerminal(e.message, 300)}` })
    }
    r.head = headSha
    r.base = mergeBase
    r.baseRef = baseRef
    r.target = target
    r.lane = laneOf(target, o.source)
    const rulesFrom = o.rulesFrom ?? trustedRefFor(target, git.resolves)
    let rulesSha
    try {
      rulesSha = git(['rev-parse', '--verify', `${rulesFrom}^{commit}`])
    } catch {
      return finish('not-reviewed', 'setup', { reasonText: `the rules ref ${rulesFrom} does not resolve` })
    }
    r.rulesFrom = rulesFrom
    r.trusted = o.trustedName ?? (/^[0-9a-f]{40}$/.test(rulesFrom) ? trustedRefFor(target, git.resolves) : rulesFrom).replace(/^origin\//, '')
    if (r.lane === 'skip') return finish('skipped', 'lane')

    const tmp = o.out ?? mkdtempSync(join(tmpdir(), 'judge-review-'))
    mkdirSync(tmp, { recursive: true })
    r.judgePr = env.judgePr ?? runJudgePr({ cwd: git.cwd, scriptDir: o.scriptDir ?? SCRIPT_DIR, base: baseRef, head: headSha, target, source: o.source, body: o.body, tmp })
    progress('judge-pr', r.judgePr.couldNotRun ? 'could not run' : `${r.judgePr.ids.length} finding(s)`)

    const prompt = parsePrompt(readFileSync(join(o.scriptDir ?? SCRIPT_DIR, 'judge-review.prompt.md'), 'utf8'))
    const prior = o.prior ?? { latest: null, samePush: [] }
    const ctx = buildContext({
      git,
      head: headSha,
      mergeBase,
      rulesSha,
      trustedName: r.trusted,
      target,
      source: o.source,
      title: o.title,
      body: o.body,
      authorKind: o.authorKind,
      judgePr: r.judgePr,
      prior: prior.latest ? { review: prior.latest.review } : null,
      budgetTokens: o.budgetTokens ?? BUDGET_TOKENS,
      prompt,
    })
    r.context = contextPlan(ctx)
    r.totals = ctx.totals
    r.contextHash = contextHash(ctx, { version: o.reviewerVersion ?? reviewerVersion(o.scriptDir), model: r.model.requested })
    r.prior = prior.latest ? { run: prior.latest.run, head: prior.latest.review.head, gateHeads: (prior.latest.review.gateHeadList ?? []).filter((h) => h !== short(headSha)).length } : null
    if (ctx.empty) return finish('skipped', 'empty')
    if (ctx.tooLarge) return finish('not-reviewed', 'too-large', { reasonText: `${REASON_TEXT['too-large']}: the fixed sections alone are ~${ctx.fixedTokens} tokens of a ${ctx.budgetTokens} budget` })
    if (o.printPrompt) {
      const [sys, user] = fullMessages(ctx, '{nonce}')
      writeFileSync(o.printPrompt, `===== system =====\n${sys}\n\n===== user =====\n${user}\n`)
    }
    if (o.dryRun) return finish('not-reviewed', o.bootstrap ? 'no-reviewer-on-trusted' : 'dry-run', { dryRun: true })

    // Reuse: an earlier review of exactly this context costs nothing again.
    const answers = parseAnswers(o.comments ?? [], o.prAuthor ?? null)
    const priorFindings = prior.latest?.review?.findings ?? []
    const reusable = [prior.latest, ...prior.samePush].filter(Boolean).find((p) => p.review.version === 2 && p.review.status === 'reviewed' && p.review.contextHash === r.contextHash)
    let raw
    let reviewOut = null
    if (reusable) {
      progress('reuse', `run ${reusable.run} reviewed this exact context`)
      r.reused = { fromRun: reusable.run, fromHead: reusable.review.head }
      r.model = { ...reusable.review.model }
      r.summary = reusable.review.summary ?? ''
      r.verificationGap = reusable.review.verificationGap ?? ''
      r.couldNotJudge = reusable.review.couldNotJudge ?? []
      raw = (reusable.review.findings ?? []).map((f) => ({ severity: f.severityRaw ?? f.severity, category: f.category, rule_id: f.ruleId, file: f.file, line: f.line, quote: f.quote, title: f.title, failure: f.failure, fix: f.fix, rule: f.rule, confidence: f.confidence }))
    } else {
      if (!env.key) return finish('not-reviewed', 'no-key')
      if (o.github && o.action === 'synchronize') {
        progress('debounce', `${DEBOUNCE_S}s before the first call — a newer push cancels this run for free`)
        await (env.sleep ?? defaultSleep)(DEBOUNCE_S * 1000, env.runSignal)
        if (env.runSignal?.aborted) return finish('not-reviewed', 'timeout', { reasonText: 'stopped during the debounce' })
      }
      const nonce = makeNonce(fullMessages(ctx, '').join('\n'))
      const [system, user] = fullMessages(ctx, nonce)
      const state = env.state ?? callState(o.ceiling ?? COST_CEILING)
      r.usage.calls = state.calls
      const callEnv = { key: env.key, state, fetchImpl: env.fetchImpl, sleep: env.sleep, now, deadlineAt: env.deadlineAt ?? t0 + 900_000, runSignal: env.runSignal, limits: env.limits ?? LIMITS, onRecord: env.onRecord }
      progress('review', `calling ${r.model.requested} (~${r.context.inputTokensEst} input tokens)`)
      let res
      try {
        res = await callModel({ ...callEnv, purpose: 'review', body: requestBody({ model: r.model.requested, purpose: 'review', messages: [{ role: 'system', content: system }, { role: 'user', content: user }] }), validate: validateReview })
      } catch (e) {
        if (e instanceof OfflineError) return finish('not-reviewed', 'offline')
        throw e
      } finally {
        r.usage.cost = state.cost
        r.usage.ceilingHit = state.ceilingHit
      }
      if (!res.ok) {
        if (res.raw && o.out) writeFileSync(join(o.out, `raw-${state.billed.review}.txt`), String(res.raw))
        return finish('not-reviewed', res.reason, { reasonText: `${REASON_TEXT[res.reason] ?? res.reason}${res.text ? `: ${res.text}` : ''}` })
      }
      reviewOut = res
      const last = res.call
      r.model.served = last.model
      r.model.provider = last.provider
      r.summary = res.value.summary
      r.verificationGap = res.value.verification_gap
      r.couldNotJudge = res.value.could_not_judge.slice(0, 3)
      raw = res.value.findings
      r._verify = { system, ctx, nonce, callEnv }
    }

    // Filters, answers, verify, tiers, the gate.
    const { kept, dropped } = filterFindings(raw, { info: ctx.info, rulesSent: ctx.rules.map((x) => x.path), filePaths: ctx.files.map((f) => f.path) })
    r.dropped = dropped
    applyAnswers(kept, answers, priorFindings)
    const deadIds = deadCallIds(ctx.xrefs)
    const priorGateHeads = r.prior?.gateHeads ?? 0
    const lane = r.lane
    const candidates = kept.filter((f) => f.severity !== 'minor' && !f.answered)
    for (const f of kept) {
      f.verified = 'n/a'
      f.verifySamples = []
      f.confirmedBy = 0
    }
    if (reusable) {
      const before = new Map((reusable.review.findings ?? []).map((x) => [x.id, x]))
      for (const f of candidates) {
        const src = before.get(f.id)
        f.verified = src?.verified && src.verified !== 'n/a' ? src.verified : 'skipped'
        f.confirmedBy = src?.confirmedBy ?? 0
        f.verifySamples = src?.verifySamples ?? []
      }
    } else if (candidates.length && !o.noVerify) {
      const gateCands = candidates.filter((f) => gateBlockers(f, { lane, priorGateHeads, deadIds, phase: 'pre' }).length === 0)
      const samples = gateCands.length ? 2 : 1
      const nonce2 = makeNonce(fullMessages(ctx, '').join('\n'))
      const [system, user] = fullMessages(ctx, r._verify.nonce, { task: 'verify', candidates: renderCandidates(candidates), nonce2 })
      const body = requestBody({ model: r.model.requested, purpose: 'verify', messages: [{ role: 'system', content: system }, { role: 'user', content: user }] })
      progress('verify', `${candidates.length} candidate(s), ${samples} blind sample(s)`)
      const runs = await Promise.all(
        Array.from({ length: samples }, () =>
          callModel({ ...r._verify.callEnv, purpose: 'verify', body, validate: (t) => validateVerify(t, candidates.length) }).catch((e) => ({ ok: false, reason: e instanceof OfflineError ? 'offline' : 'crash', text: String(e?.message ?? e) })),
        ),
      )
      const state = r._verify.callEnv.state
      r.usage.cost = state.cost
      r.usage.ceilingHit = state.ceilingHit
      const ok = runs.filter((x) => x.ok)
      candidates.forEach((f, i) => {
        f.verifySamples = ok.map((x) => x.value[i])
        const v = combineVerdicts(f.verifySamples)
        f.verified = v.verified
        f.confirmedBy = v.confirmedBy
      })
    } else {
      for (const f of candidates) f.verified = 'skipped'
    }
    // Refuted drops; all-unsure demotes one level.
    const survivors = []
    for (const f of kept) {
      if (f.verified === 'refuted') {
        r.dropped.push({ reason: 'refuted', evidence: f.verifySamples.map((s) => s?.evidence).filter(Boolean).join(' / ').slice(0, 600), finding: f })
        continue
      }
      if (f.verified === 'unsure') f.severity = f.severity === 'blocker' ? 'major' : 'minor'
      survivors.push(f)
    }
    assignTiers(survivors)
    markGate(survivors, { mode: o.gateMode ?? 'report', lane, priorGateHeads, deadIds })
    r.findings = survivors
    const priorList = (prior.latest?.review?.gateHeadList ?? []).filter((h) => h !== short(headSha))
    r.gateHeadList = survivors.some((f) => f.gate || f.wouldGate) ? [...priorList, short(headSha)] : priorList
    r.gateHeads = r.gateHeadList.length
    if (reusable) r.usage.cost = 0
    delete r._verify
    void reviewOut
    return finish('reviewed', null, { reasonText: '' })
  } catch (e) {
    delete r._verify
    return finish('not-reviewed', 'crash', { reasonText: `the reviewer crashed: ${safeTerminal(String(e?.stack ?? e), 600)}` })
  }
}

/** review.json, comment.md, comment-action and summary.md. */
export function writeOutputs(r, dir, { repo } = {}) {
  mkdirSync(dir, { recursive: true })
  const json = JSON.stringify(r, (k, v) => (v instanceof Set || v instanceof Map ? undefined : k === 'judgePr' && v ? { exit: v.exit, counts: v.counts, ids: v.ids, couldNotRun: Boolean(v.couldNotRun) } : v), 2)
  writeFileSync(join(dir, 'review.json'), json)
  const comment = renderComment(r, { repo })
  writeFileSync(join(dir, 'comment.md'), comment)
  writeFileSync(join(dir, 'comment-action'), r.status === 'reviewed' && r.findings.some((f) => f.tier === 'A') ? 'create' : 'update')
  const summary = renderSummary(r)
  writeFileSync(join(dir, 'summary.md'), summary)
  return { comment, summary }
}

/** The plan a dry run prints. */
export function renderPlan(r) {
  const c = r.context ?? {}
  const est = c.inputTokensEst ?? 0
  const lo = (est * 0.0525 + 10_000 * 0.25) / 1e6
  const hi = (est * 0.15 + 20_000 * 0.5) / 1e6
  const lines = [
    `## judge (review) — context plan for \`${short(r.head)}\` vs \`${slug(r.target)}\``,
    '',
    `- reviewer: ${r.reason === 'no-reviewer-on-trusted' ? "the head's own copy (bootstrap), no key" : 'this tree'}; the key it looks for: \`${KEY_ENV}\``,
    `- files: ${c.filesShown ?? 0} shown of ${c.files ?? 0}${c.partial ? ` (partial: ${(c.filesNotShown ?? []).length} not shown)` : ''}; diff at -U${c.diffContext ?? 25}`,
    `- rules: ${(c.rules ?? []).join(', ') || '—'}`,
    `- reference: ${(c.reference ?? []).join(', ') || '—'}`,
    `- trims: ${(c.trims ?? []).join(', ') || 'none'}`,
    `- input estimate: ~${est} tokens of ${BUDGET_TOKENS}`,
    `- estimated cost: $${lo.toFixed(4)}–$${hi.toFixed(4)} for the review call, plus a verify call when a finding survives`,
    '',
    '| section | bytes | ~tokens |',
    '| --- | --- | --- |',
    ...Object.entries(c.sections ?? {}).map(([k, v]) => `| ${k} | ${v} | ${Math.ceil(v / 3)} |`),
  ]
  return `${lines.join('\n')}\n`
}

// ── fetch-prior ─────────────────────────────────────────────────────────────

/** Was a run made under the trusted workflow file? Its head's
 *  judge-review.yml blob must be the trusted tree's. */
export function sameWorkflow(git, sha, trustedBlob) {
  if (!git.resolves(sha)) git(['fetch', '--no-tags', '--quiet', 'origin', sha], { allowFail: true, timeout: 60_000 })
  return git(['rev-parse', `${sha}:.github/workflows/judge-review.yml`], { allowFail: true }) === trustedBlob
}

/** Earlier reviews of this pull request or head, whose workflow file is the
 *  trusted one. Any failure is an empty index — never a failed job. */
export function fetchPrior(o) {
  const out = o.out
  mkdirSync(out, { recursive: true })
  const index = { latestPr: null, samePush: [] }
  try {
    const git = makeGit(process.cwd())
    const trustedBlob = git(['rev-parse', `${o.trusted}:.github/workflows/judge-review.yml`], { allowFail: true })
    if (!trustedBlob) throw new Error('the trusted tree has no judge-review.yml')
    const list = (q) => {
      const r = gh(['api', `repos/${o.repo}/actions/workflows/judge-review.yml/runs?${q}`, '--jq', '.workflow_runs'])
      return r.ok ? JSON.parse(r.out || '[]') : []
    }
    const sameBlob = (sha) => sameWorkflow(git, sha, trustedBlob)
    const download = (id) => {
      const r = gh(['run', 'download', String(id), '-n', 'judge-review', '-D', join(out, String(id)), '-R', o.repo], { timeout: 60_000 })
      return r.ok && existsSync(join(out, String(id), 'review.json'))
    }
    if (o.pr && o.pr !== 'none') {
      const runs = list('event=pull_request&status=completed&per_page=100')
        .filter((x) => String(x.id) !== String(o['run-id']) && (x.pull_requests ?? []).some((p) => String(p.number) === String(o.pr)))
        .sort((a, b) => b.id - a.id)
      for (const run of runs.slice(0, 5)) {
        if (sameBlob(run.head_sha) && download(run.id)) {
          index.latestPr = { run: String(run.id), head: run.head_sha }
          break
        }
      }
    }
    const pushes = list(`event=push&head_sha=${encodeURIComponent(o.head)}&per_page=20`).filter((x) => x.status === 'completed' && String(x.id) !== String(o['run-id']))
    for (const run of pushes.slice(0, 3)) if (sameBlob(run.head_sha) && download(run.id)) index.samePush.push(String(run.id))
  } catch (e) {
    progress('prior', `none read: ${e.message}`)
  }
  writeFileSync(join(out, 'index.json'), JSON.stringify(index, null, 2))
  progress('prior', `latest PR review: ${index.latestPr?.run ?? 'none'}; same-head push reviews: ${index.samePush.length}`)
  return index
}

// ── calibration, scoring, ledger, canary ────────────────────────────────────

export const CALIBRATION_KINDS = ['dev-defect', 'heldout-defect', 'clean-small', 'clean-real', 'optional']
const DEFAULT_SAMPLES = { 'dev-defect': 2, 'heldout-defect': 2, 'clean-small': 3, 'clean-real': 3, optional: 1 }

export function loadSet(path = join(SCRIPT_DIR, 'judge-review.calibration.json')) {
  return JSON.parse(readFileSync(path, 'utf8'))
}

export function selectEntries(set, only) {
  const all = set.entries
  if (!only) return all.filter((e) => e.kind !== 'optional')
  if (only === 'dev') return all.filter((e) => e.kind === 'dev-defect')
  if (only === 'heldout') return all.filter((e) => e.kind === 'heldout-defect')
  if (only === 'clean') return all.filter((e) => e.kind.startsWith('clean'))
  const prs = String(only).split(/[\s,]+/).filter(Boolean).map(Number)
  return all.filter((e) => prs.includes(e.pr))
}

/** Does a Tier A finding hit a defect? File regex, line range ±10, words in
 *  the title or failure. The quote is never matched. */
export function isHit(f, d) {
  if (!new RegExp(d.file).test(f.file)) return false
  if (Array.isArray(d.lines)) {
    const [s, e] = d.lines
    if (f.endLine < s - 10 || f.line > e + 10) return false
  }
  return new RegExp(d.words, 'i').test(`${f.title} ${f.failure}`)
}

const median = (xs) => {
  if (!xs.length) return 0
  const s = [...xs].sort((a, b) => a - b)
  return s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2
}
const pct = (xs, p) => {
  if (!xs.length) return 0
  const s = [...xs].sort((a, b) => a - b)
  return s[Math.min(s.length - 1, Math.ceil((p / 100) * s.length) - 1)]
}

/** The scorecard from calibration results: [{entry, sample, review}]. */
export function scoreResults(set, results, grades = { calibration: {} }) {
  const byPr = new Map(set.entries.map((e) => [e.pr, e]))
  const runs = results.filter((x) => byPr.has(x.pr))
  const gradeOf = (pr, f) => grades.calibration?.[`${pr}:${f.id}:${f.category}`] ?? null
  const entries = []
  for (const e of set.entries) {
    const mine = runs.filter((x) => x.pr === e.pr)
    if (!mine.length) continue
    const hits = []
    const unmatched = []
    const would = []
    for (const x of mine) {
      for (const f of (x.review.findings ?? []).filter((y) => y.tier === 'A')) {
        const d = (e.defects ?? []).find((dd) => isHit(f, dd))
        if (d) hits.push({ sample: x.sample, defect: d.key, id: f.id, title: f.title, failure: f.failure, grade: gradeOf(e.pr, f) })
        else unmatched.push({ sample: x.sample, id: f.id, category: f.category, file: f.file, line: f.line, title: f.title, grade: gradeOf(e.pr, f) })
      }
      for (const f of (x.review.findings ?? []).filter((y) => y.wouldGate)) would.push({ sample: x.sample, id: f.id, category: f.category, title: f.title, hit: (e.defects ?? []).some((dd) => isHit(f, dd)), grade: gradeOf(e.pr, f) })
    }
    const misses = (e.defects ?? []).filter((d) => !hits.some((h) => h.defect === d.key)).map((d) => d.key)
    entries.push({ pr: e.pr, kind: e.kind, runs: mine.length, hitRuns: new Set(hits.map((h) => h.sample)).size, hits, misses, unmatched, would })
  }
  const kindRuns = (k) => runs.filter((x) => byPr.get(x.pr).kind === k || (k === 'clean' && byPr.get(x.pr).kind.startsWith('clean')))
  const reviewed = runs.filter((x) => x.review.status === 'reviewed')
  const walls = reviewed.map((x) => (x.review.usage?.ms ?? 0) / 1000)
  const allCalls = runs.flatMap((x) => x.review.usage?.calls ?? [])
  const R1 = { reviewedRate: runs.length ? reviewed.length / runs.length : 0, p90WallS: pct(walls, 90), everyCallCosted: allCalls.every((c) => typeof c.cost === 'number') }
  R1.pass = R1.reviewedRate >= 0.95 && R1.p90WallS <= 360 && R1.everyCallCosted
  const cleanRuns = kindRuns('clean')
  const tierA = (x) => (x.review.findings ?? []).filter((f) => f.tier === 'A')
  const cleanReal = kindRuns('clean-real').map((x) => tierA(x).length)
  const N1 = { cleanBlockers: cleanRuns.reduce((n, x) => n + tierA(x).filter((f) => f.severity === 'blocker').length, 0), cleanRealMedian: median(cleanReal), cleanRealP90: pct(cleanReal, 90) }
  N1.pass = N1.cleanBlockers === 0 && N1.cleanRealMedian === 0 && N1.cleanRealP90 <= 1
  const allA = runs.flatMap((x) => tierA(x).map((f) => gradeOf(x.pr, f)))
  const graded = allA.filter((g) => g)
  const P1 = { tierA: allA.length, graded: graded.length, real: graded.filter((g) => g.real).length }
  P1.precision = P1.graded ? P1.real / P1.graded : null
  P1.pass = P1.graded === P1.tierA && (P1.tierA === 0 || P1.precision >= 0.6)
  const recall = (kind) => {
    const es = entries.filter((x) => x.kind === kind)
    const rs = es.reduce((n, x) => n + x.runs, 0)
    return { prsHit: es.filter((x) => x.hitRuns > 0).length, prs: es.length, perRun: rs ? es.reduce((n, x) => n + x.hitRuns, 0) / rs : 0 }
  }
  const dev = recall('dev-defect')
  const held = recall('heldout-defect')
  const R3 = { ...dev, pass: dev.prsHit >= 4 && dev.perRun >= 0.5 }
  const H1 = { ...held, overfit: dev.perRun * 100 - held.perRun * 100 > 35, pass: held.prsHit >= 1 }
  const costs = reviewed.map((x) => x.review.usage?.cost ?? 0)
  const R4 = { median: median(costs), max: costs.length ? Math.max(...costs) : 0 }
  R4.pass = R4.median <= 0.04 && R4.max <= 0.15
  const conf = { high: 0, medium: 0 }
  for (const x of reviewed) for (const f of x.review.findings ?? []) if (f.confidence in conf) conf[f.confidence]++
  const C1 = { ...conf, highShare: conf.high + conf.medium ? conf.high / (conf.high + conf.medium) : 0 }
  C1.flag = C1.highShare > 0.8
  const G1 = { wouldOnClean: cleanRuns.reduce((n, x) => n + (x.review.findings ?? []).filter((f) => f.wouldGate).length, 0) }
  G1.pass = G1.wouldOnClean === 0
  const blockerPrs = [710, 682, 686, 581]
  const G3 = { prs: entries.filter((x) => blockerPrs.includes(x.pr) && x.would.some((w) => w.hit)).map((x) => x.pr) }
  G3.pass = G3.prs.length >= 2
  const metrics = { R1, N1, P1, R3, H1, R4, C1, G1, G3 }
  return { entries, metrics, keepOn: R1.pass && R3.pass && R4.pass && N1.pass }
}

export function renderScorecard(card) {
  const m = card.metrics
  const yes = (b) => (b ? 'pass' : 'MISS')
  const out = [
    '## judge (review) — calibration scorecard',
    '',
    '| bar | value | result |',
    '| --- | --- | --- |',
    `| R1 reliability | ${(m.R1.reviewedRate * 100).toFixed(0)}% reviewed, p90 ${m.R1.p90WallS.toFixed(0)}s, every call costed: ${m.R1.everyCallCosted} | ${yes(m.R1.pass)} |`,
    `| N1 noise | ${m.N1.cleanBlockers} clean blockers; clean-real Tier A median ${m.N1.cleanRealMedian}, p90 ${m.N1.cleanRealP90} | ${yes(m.N1.pass)} |`,
    `| P1 precision | ${m.P1.real}/${m.P1.graded} graded real of ${m.P1.tierA} Tier A | ${m.P1.graded < m.P1.tierA ? 'needs grades' : yes(m.P1.pass)} |`,
    `| R3 dev recall | ${m.R3.prsHit}/${m.R3.prs} PRs, per-run ${(m.R3.perRun * 100).toFixed(0)}% | ${yes(m.R3.pass)} |`,
    `| H1 held-out | ${m.H1.prsHit}/${m.H1.prs} PRs, per-run ${(m.H1.perRun * 100).toFixed(0)}% (dev ${(m.R3.perRun * 100).toFixed(0)}%)${m.H1.overfit ? ' — **overfit**' : ''} | ${yes(m.H1.pass)} |`,
    `| R4 cost | median $${m.R4.median.toFixed(4)}, max $${m.R4.max.toFixed(4)} | ${yes(m.R4.pass)} |`,
    `| C1 confidence | high ${m.C1.high}, medium ${m.C1.medium} (${(m.C1.highShare * 100).toFixed(0)}% high) | ${m.C1.flag ? 'set GATE_NEEDS_HIGH=false' : 'discriminates'} |`,
    `| G1 clean would-gate | ${m.G1.wouldOnClean} | ${yes(m.G1.pass)} |`,
    `| G3 blocker would-gate hits | ${m.G3.prs.join(', ') || 'none'} | ${yes(m.G3.pass)} |`,
    '',
    `Keep the reviewer on (R1, R3, R4, N1): **${card.keepOn ? 'yes' : 'no'}**`,
  ]
  for (const e of card.entries) {
    out.push('', `### #${e.pr} (${e.kind}) — ${e.hitRuns}/${e.runs} runs hit${e.misses.length ? `; missed ${e.misses.join(', ')}` : ''}`)
    for (const h of e.hits) out.push(`- hit ${h.defect} (sample ${h.sample}) ${h.id}: ${sanitizeText(h.title, 100)} — ${h.grade ? (h.grade.real ? 'graded real' : 'graded false') : 'to confirm'}`)
    for (const u of e.unmatched) out.push(`- unmatched ${u.id} ${u.category} ${renderPath(`${u.file}:${u.line}`)}: ${sanitizeText(u.title, 100)} — ${u.grade ? (u.grade.real ? 'graded real' : 'graded false') : 'to grade'}`)
    for (const w of e.would) out.push(`- would-gate ${w.id} ${w.category}${w.hit ? ' (a hit)' : ''}: ${sanitizeText(w.title, 100)} — ${w.grade ? (w.grade.real ? 'graded real' : 'graded false') : 'to grade'}`)
  }
  return `${out.join('\n')}\n`
}

/** The G2 counters from a ledger. */
export function scoreLedger(ledger, grades = { live: {} }) {
  const heads = ledger.reviews ?? []
  const reviewed = heads.filter((x) => x.status === 'reviewed')
  const would = reviewed.flatMap((x) => (x.findings ?? []).filter((f) => f.wouldGate || f.gate).map((f) => ({ ...f, key: `${x.pr}:${short(x.head)}:${f.id}`, pr: x.pr })))
  const graded = would.map((f) => ({ ...f, grade: grades.live?.[f.key] ?? null }))
  const done = graded.filter((f) => f.grade)
  const falsePrs = new Set(done.filter((f) => !f.grade.real).map((f) => f.pr))
  const prs = new Set(reviewed.map((x) => x.pr))
  const G2 = {
    reviewedHeads: reviewed.length,
    wouldGate: would.length,
    graded: done.length,
    precision: done.length ? done.filter((f) => f.grade.real).length / done.length : null,
    falseHoldPrs: falsePrs.size,
    falseHoldRate: prs.size ? falsePrs.size / prs.size : 0,
  }
  G2.pass = G2.reviewedHeads >= 150 && G2.graded === G2.wouldGate && (G2.wouldGate === 0 || G2.precision >= 0.8) && G2.falseHoldRate <= 0.03
  return { G2, pending: graded.filter((f) => !f.grade).map((f) => f.key) }
}

async function pool(items, n, fn) {
  const out = new Array(items.length)
  let i = 0
  await Promise.all(
    Array.from({ length: Math.min(n, items.length) }, async () => {
      while (i < items.length) {
        const k = i++
        out[k] = await fn(items[k], k)
      }
    }),
  )
  return out
}

function readResults(dir) {
  const out = []
  for (const d of existsSync(dir) ? readdirSync(dir, { withFileTypes: true }) : []) {
    if (!d.isDirectory()) continue
    const m = /^(\d+)-(\d+)$/.exec(d.name)
    if (!m) continue
    const f = join(dir, d.name, 'review.json')
    if (!existsSync(f)) continue
    try {
      out.push({ pr: Number(m[1]), sample: Number(m[2]), review: JSON.parse(readFileSync(f, 'utf8')) })
    } catch {
      // a half-written result is skipped
    }
  }
  return out
}

function readGrades(path) {
  try {
    return JSON.parse(readFileSync(path ?? join(SCRIPT_DIR, 'judge-review.grades.json'), 'utf8'))
  } catch {
    return { version: 1, calibration: {}, live: {} }
  }
}

// ── canary fixtures ─────────────────────────────────────────────────────────

/** A scratch repository with a fixture's base and head commits. */
export function fixtureRepo(fixtureDir) {
  const dir = mkdtempSync(join(tmpdir(), 'judge-canary-'))
  const run = (...args) => execFileSync('git', ['-c', 'user.name=judge', '-c', 'user.email=judge@example.invalid', '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null', ...args], { cwd: dir, env: childEnv(), stdio: ['ignore', 'pipe', 'pipe'], encoding: 'utf8' }).trim()
  const copy = (from) => {
    for (const e of readdirSync(from, { withFileTypes: true, recursive: true })) {
      if (!e.isFile()) continue
      const src = join(e.parentPath ?? e.path, e.name)
      const rel = src.slice(from.length + 1)
      mkdirSync(dirname(join(dir, rel)), { recursive: true })
      writeFileSync(join(dir, rel), readFileSync(src))
    }
  }
  run('init', '-q', '-b', 'rc')
  copy(join(fixtureDir, 'base'))
  const rules = join(fixtureDir, 'rules.md')
  writeFileSync(join(dir, 'AGENTS.md'), existsSync(rules) ? readFileSync(rules) : '# AGENTS.md\n\nNo rules for this fixture.\n')
  writeFileSync(join(dir, 'CONTRIBUTING.md'), '# CONTRIBUTING.md\n\nThe fixture has no contribution rules.\n')
  run('add', '-A')
  run('commit', '-q', '-m', 'base: the fixture — canary')
  const base = run('rev-parse', 'HEAD')
  run('update-ref', 'refs/remotes/origin/rc', base)
  run('switch', '-q', '-c', 'canary/change')
  for (const e of readdirSync(dir)) if (!['.git', 'AGENTS.md', 'CONTRIBUTING.md'].includes(e)) rmSync(join(dir, e), { recursive: true, force: true })
  copy(join(fixtureDir, 'head'))
  run('add', '-A')
  run('commit', '-q', '-m', 'canary: the change — canary')
  return { dir, base, head: run('rev-parse', 'HEAD') }
}

/** Is a canary sample a pass? defect: a Tier A security finding on the
 *  seeded file; clean: no Tier A at all. */
export function canaryVerdict(kind, reviews, seededFile) {
  const valid = reviews.every((x) => x.status === 'reviewed' && x.usage?.calls?.every((c) => typeof c.cost === 'number'))
  if (!valid) return { pass: false, why: 'a sample did not review with a recorded cost' }
  const a = (x) => (x.findings ?? []).filter((f) => f.tier === 'A')
  if (kind === 'defect') {
    const hit = reviews.some((x) => a(x).some((f) => f.category === 'security' && f.file === seededFile))
    return hit ? { pass: true, why: 'the seeded defect was listed' } : { pass: false, why: 'the seeded defect was missed in every sample' }
  }
  const any = reviews.some((x) => a(x).length)
  return any ? { pass: false, why: 'the clean fixture drew a Tier A finding' } : { pass: true, why: 'the clean fixture drew nothing to read' }
}

/** The 7-day not-reviewed rate from verdicts: >20% for a reason that is
 *  not expected is a dead judge. */
export function healthOf(verdicts) {
  const counted = verdicts.filter(Boolean)
  const bad = counted.filter((v) => v.status === 'not-reviewed' && !SKIP_REASONS_FOR_HEALTH.has(v.reason))
  const rate = counted.length ? bad.length / counted.length : 0
  return { runs: counted.length, bad: bad.length, rate, pass: rate <= 0.2 }
}

// ── the command line ────────────────────────────────────────────────────────

function readJsonFile(path) {
  try {
    return JSON.parse(readFileSync(path, 'utf8'))
  } catch {
    return null
  }
}

function usageError(why) {
  process.stderr.write(`judge-review: could not run — usage: ${why}\n`)
  return 1
}

/** `review` from the command line: the options, the key, the outputs, the exit. */
async function cliReview(opts) {
  const githubMode = Boolean(opts.github)
  const stopToken = githubMode ? randomBytes(16).toString('hex') : null
  if (githubMode) process.stdout.write(`::stop-commands::${stopToken}\n`)
  if (opts.replay && process.env.GITHUB_ACTIONS) {
    if (githubMode) process.stdout.write(`::${stopToken}::\n`)
    return usageError('--replay is test-only and refused under GitHub Actions')
  }
  const key = opts['dry-run'] ? (takeKey('__none__'), null) : takeKey(KEY_ENV)
  const offline = process.env.JUDGE_REVIEW_OFFLINE === '1'
  const deadlineS = Number(opts.deadline ?? (githubMode ? 600 : 900))
  const deadlineAt = PROCESS_START + deadlineS * 1000
  const runCtl = new AbortController()
  let signalExit = null
  const onSig = (sig, code) => () => {
    signalExit = code
    runCtl.abort(new Error(sig))
  }
  process.once('SIGINT', onSig('SIGINT', 130))
  process.once('SIGTERM', onSig('SIGTERM', 143))
  const deadlineTimer = setTimeout(() => runCtl.abort(new Error('deadline')), Math.max(1, deadlineAt - Date.now()))
  deadlineTimer.unref?.()

  // The pull request, from the API (CI) or from gh (local --pr).
  let title = opts.title ?? null
  let body = opts['pr-body'] ? (existsSync(opts['pr-body']) ? readFileSync(opts['pr-body'], 'utf8') : '') : null
  let comments = opts['comments-json'] ? readJsonFile(opts['comments-json']) ?? [] : []
  let prAuthor = null
  let authorKind = 'unknown'
  let pr = opts.pr ?? null
  const prJson = opts['pr-json'] ? readJsonFile(opts['pr-json']) : null
  if (prJson) {
    title = title ?? prJson.title ?? ''
    body = body ?? prJson.body ?? ''
    prAuthor = prJson.user?.login ?? null
    authorKind = prJson.user?.type === 'Bot' ? 'bot' : 'user'
    pr = pr ?? String(prJson.number ?? '')
  } else if (opts.pr && !githubMode) {
    const v = gh(['pr', 'view', String(opts.pr), '--json', 'title,body,author,number'])
    if (v.ok) {
      const j = JSON.parse(v.out)
      title = title ?? j.title
      body = body ?? j.body
      prAuthor = j.author?.login ?? null
      authorKind = j.author?.is_bot ? 'bot' : 'user'
      const c = gh(['api', `repos/{owner}/{repo}/issues/${opts.pr}/comments`, '--paginate', '--slurp'])
      if (c.ok) comments = JSON.parse(c.out || '[]')
    }
  }
  const git = makeGit(process.cwd())
  if (title === null) title = git(['log', '-1', '--format=%s', opts.head ?? 'HEAD'], { allowFail: true }) ?? ''
  const base = opts.base ?? (process.env.GITHUB_BASE_REF ? `origin/${process.env.GITHUB_BASE_REF}` : 'auto')
  const gateMode = gateModeOf()
  const model = opts.model ?? (process.env.JUDGE_REVIEW_MODEL?.trim() || DEFAULT_MODEL)
  const out = opts.out ?? join(tmpdir(), `judge-review-${Date.now()}`)
  mkdirSync(out, { recursive: true })
  let fetchImpl = globalThis.fetch
  if (opts.replay) fetchImpl = replayFetch(opts.replay)
  else if (offline) fetchImpl = async () => {
    throw new OfflineError('network refused (offline)')
  }
  const onRecord = opts.record
    ? (purpose, attempt, data) => {
        mkdirSync(opts.record, { recursive: true })
        writeFileSync(join(opts.record, `${purpose}-${attempt}.sse`), recordable(data))
      }
    : null
  const r = await review(
    {
      base,
      head: opts.head ?? 'HEAD',
      target: opts.target ?? 'auto',
      source: opts.source ?? null,
      rulesFrom: opts['rules-from'] ?? null,
      title,
      body,
      comments,
      prAuthor,
      authorKind,
      pr,
      prior: loadPrior(opts.prior),
      event: opts.event ?? null,
      action: opts.action ?? null,
      github: githubMode,
      out,
      dryRun: Boolean(opts['dry-run']),
      bootstrap: Boolean(opts.bootstrap),
      printPrompt: opts['print-prompt'] ?? null,
      noVerify: Boolean(opts['no-verify']),
      model,
      budgetTokens: opts['budget-tokens'] ? Number(opts['budget-tokens']) : BUDGET_TOKENS,
      gateMode,
    },
    { key, fetchImpl, deadlineAt, runSignal: runCtl.signal, onRecord },
  )
  clearTimeout(deadlineTimer)
  const { summary } = writeOutputs(r, out, { repo: process.env.GITHUB_REPOSITORY || 'outcrop-labs/talaria' })
  const plan = r.context ? renderPlan(r) : ''
  if (githubMode && process.env.GITHUB_STEP_SUMMARY) {
    try {
      appendFileSync(process.env.GITHUB_STEP_SUMMARY, r.dryRun ? `${plan}\n${summary}` : summary)
    } catch {
      // the summary is a convenience
    }
  }
  if (githubMode) {
    process.stdout.write(`::${stopToken}::\n`)
    for (const line of annotations(r)) process.stdout.write(`${line}\n`)
  } else if (opts.json) {
    process.stdout.write(readFileSync(join(out, 'review.json'), 'utf8'))
  } else {
    process.stdout.write(r.dryRun ? `${r.judgePr?.report ? `${r.judgePr.report.trim()}\n\n` : ''}${plan}` : renderLocal(r))
  }
  const a = r.findings.filter((f) => f.tier === 'A').length
  const b = r.findings.filter((f) => f.tier === 'B').length
  const c = r.findings.filter((f) => f.tier === 'C').length
  const dryNote = r.dryRun ? ` · dry run (looked for ${KEY_ENV})` : ''
  process.stderr.write(
    `judge-review: ${r.status === 'reviewed' ? 'reviewed' : r.status}${r.reason ? ` (${r.reason})` : ''} ${short(r.head)} · ${a} to read, ${b} more, ${c} minor · $${fmtCost(r.usage.cost)} · ${slug(r.model.served ?? r.model.requested)} via ${slug(r.model.provider)} · ${fmtMs(r.usage.ms)}${dryNote} · out ${out}\n`,
  )
  if (signalExit) return signalExit
  if (githubMode) return 0
  if (r.dryRun) return r.reason === 'setup' ? 1 : 0
  if (r.status === 'reviewed') return a ? 2 : 0
  if (r.status === 'skipped') return 0
  return 1
}

async function cliCalibrate(opts) {
  const key = takeKey(CALIBRATION_KEY_ENV)
  const samplesArg = opts.samples ? Number(opts.samples) : null
  if (samplesArg !== null && !(Number.isInteger(samplesArg) && samplesArg >= 1 && samplesArg <= 3)) return usageError('--samples takes 1 to 3')
  const set = loadSet(opts.set)
  const entries = selectEntries(set, opts.only || null)
  if (!entries.length) return usageError(`--only ${opts.only} names no entry`)
  if (!key) {
    process.stderr.write(`judge-review: could not run — no ${CALIBRATION_KEY_ENV}\n`)
    return 1
  }
  const out = opts.out ?? join(tmpdir(), `judge-calibration-${Date.now()}`)
  mkdirSync(out, { recursive: true })
  const model = opts.model ?? (process.env.JUDGE_REVIEW_MODEL?.trim() || DEFAULT_MODEL)
  const ceiling = opts.model ? CALIBRATION_MODEL_CEILING : COST_CEILING
  const deadlineAt = Date.now() + 50 * 60_000
  const jobs = entries.flatMap((e) => Array.from({ length: samplesArg ?? e.samples ?? DEFAULT_SAMPLES[e.kind] ?? 1 }, (_, k) => ({ e, k: k + 1 })))
  const results = []
  const grades = readGrades(opts.grades)
  const write = () => {
    const card = scoreResults(set, results, grades)
    writeFileSync(join(out, 'scorecard.json'), JSON.stringify(card, null, 2))
    writeFileSync(join(out, 'scorecard.md'), renderScorecard(card))
    return card
  }
  QUIET = true
  await pool(jobs, Number(opts.concurrency ?? 6), async ({ e, k }) => {
    if (Date.now() > deadlineAt - 5 * 60_000) return
    const dir = join(out, `${e.pr}-${k}`)
    const r = await review(
      { base: e.mergeBase, head: e.head, target: 'rc', source: e.source, rulesFrom: e.mergeBase, title: e.title, body: e.body, out: dir, model, gateMode: 'calibrate', ceiling },
      { key, deadlineAt: Math.min(deadlineAt, Date.now() + 600_000), onRecord: opts.record && k === 1 ? (purpose, attempt, data) => {
        mkdirSync(join(opts.record, String(e.pr)), { recursive: true })
        writeFileSync(join(opts.record, String(e.pr), `${purpose}-${attempt}.sse`), recordable(data))
      } : null },
    )
    writeOutputs(r, dir)
    results.push({ pr: e.pr, sample: k, review: JSON.parse(readFileSync(join(dir, 'review.json'), 'utf8')) })
    process.stderr.write(`judge-review: calibrate — #${e.pr} sample ${k}: ${r.status}${r.reason ? ` (${r.reason})` : ''}, ${r.findings.filter((f) => f.tier === 'A').length} Tier A, $${fmtCost(r.usage.cost)}\n`)
    write()
  })
  const card = write()
  const md = renderScorecard(card)
  if (opts.github && process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, md)
  process.stdout.write(md)
  return card.keepOn ? 0 : 2
}

function cliScore(opts) {
  const grades = readGrades(opts.grades)
  if (opts.ledger) {
    const ledger = readJsonFile(opts.ledger)
    if (!ledger) return usageError(`--ledger ${opts.ledger} is not readable JSON`)
    const s = scoreLedger(ledger, grades)
    process.stdout.write(`${JSON.stringify(s, null, 2)}\n`)
    return s.G2.pass ? 0 : 2
  }
  if (!opts.results) return usageError('score needs --results <dir> or --ledger <file>')
  const card = scoreResults(loadSet(opts.set), readResults(opts.results), grades)
  process.stdout.write(renderScorecard(card))
  return card.keepOn ? 0 : 2
}

function cliLedger(opts) {
  const days = Number(/^(\d+)d$/.exec(opts.since ?? '30d')?.[1] ?? 30)
  const since = new Date(Date.now() - days * 86_400_000).toISOString().slice(0, 10)
  const out = opts.out ?? join(tmpdir(), `judge-ledger-${Date.now()}`)
  mkdirSync(out, { recursive: true })
  const runs = gh(['api', '--paginate', `repos/{owner}/{repo}/actions/workflows/judge-review.yml/runs?event=pull_request&status=completed&created=>=${since}&per_page=100`, '--jq', '.workflow_runs[] | {id, head_sha, pull_requests}'], { timeout: 120_000 })
  if (!runs.ok) {
    process.stderr.write(`judge-review: could not run — the run listing failed: ${runs.err}\n`)
    return 1
  }
  const rows = runs.out.split('\n').filter(Boolean).map((l) => JSON.parse(l))
  const latest = new Map()
  for (const r of rows) {
    const pr = r.pull_requests?.[0]?.number
    if (!pr) continue
    const k = `${pr}:${r.head_sha}`
    if (!latest.has(k) || latest.get(k).id < r.id) latest.set(k, { ...r, pr })
  }
  const reviews = []
  for (const r of latest.values()) {
    const dir = join(out, String(r.id))
    const d = gh(['run', 'download', String(r.id), '-n', 'judge-review', '-D', dir], { timeout: 60_000 })
    const rv = d.ok ? readJsonFile(join(dir, 'review.json')) : null
    if (rv) reviews.push({ pr: r.pr, run: r.id, head: rv.head, status: rv.status, reason: rv.reason, findings: (rv.findings ?? []).filter((f) => f.tier === 'A' || f.wouldGate || f.gate) })
  }
  const ledger = { since, reviews, escapes: [] }
  if (opts.escapes) {
    for (const pr of [...new Set(reviews.map((x) => x.pr))].slice(0, 200)) {
      const s = gh(['api', `search/issues?q=${encodeURIComponent(`repo:outcrop-labs/talaria is:pr is:merged "#${pr}"`)}`, '--jq', '.items[] | {number, title}'])
      for (const l of s.ok ? s.out.split('\n').filter(Boolean) : []) {
        const it = JSON.parse(l)
        if (it.number !== pr && /fix|follow-?up|regress/i.test(it.title)) ledger.escapes.push({ pr, laterPr: it.number, title: it.title, listedByJudge: null })
      }
    }
  }
  writeFileSync(join(out, 'ledger.json'), JSON.stringify(ledger, null, 2))
  const s = scoreLedger(ledger, readGrades(opts.grades))
  const md = [
    `## judge (review) ledger since ${since}`,
    '',
    `G2: ${s.G2.reviewedHeads} reviewed heads, ${s.G2.wouldGate} would-gate findings, ${s.G2.graded} graded, precision ${s.G2.precision === null ? '—' : `${(s.G2.precision * 100).toFixed(0)}%`}, false-hold PRs ${s.G2.falseHoldPrs} (${(s.G2.falseHoldRate * 100).toFixed(1)}%) — ${s.G2.pass ? 'pass' : 'not yet'}`,
    '',
    ...reviews.flatMap((x) => x.findings.map((f) => `- #${x.pr} ${short(x.head)} ${f.id} ${f.severity} ${f.category}${f.wouldGate || f.gate ? ' would-gate' : ''} — ${sanitizeText(f.title, 100)} ([blob](${blobLink('outcrop-labs/talaria', x.head, f.file)}#L${f.line}))`)),
    ...(ledger.escapes.length ? ['', '### Later fixes to grade', ...ledger.escapes.map((e) => `- #${e.pr} → #${e.laterPr}: ${sanitizeText(e.title, 100)}`)] : []),
  ].join('\n')
  writeFileSync(join(out, 'ledger.md'), `${md}\n`)
  process.stdout.write(`${md}\n`)
  return 0
}

async function cliCanary(opts) {
  const key = takeKey(KEY_ENV)
  const lines = []
  let ok = true
  if (!key) {
    process.stderr.write(`judge-review: could not run — no ${KEY_ENV}\n`)
    return 1
  }
  QUIET = true
  for (const kind of ['defect', 'clean']) {
    const fx = fixtureRepo(join(SCRIPT_DIR, 'judge-review.fixtures', 'canary', kind))
    try {
      const reviews = []
      for (let k = 0; k < 2; k++) {
        reviews.push(await review({ base: 'origin/rc', head: fx.head, target: 'rc', source: 'canary/change', rulesFrom: fx.base, cwd: fx.dir, title: `canary: ${kind}`, body: '', gateMode: 'report' }, { key, git: makeGit(fx.dir), deadlineAt: Date.now() + 300_000 }))
      }
      const seeded = readJsonFile(join(SCRIPT_DIR, 'judge-review.fixtures', 'canary', kind, 'seeded.json'))
      const v = canaryVerdict(kind, reviews, seeded?.file ?? null)
      ok = ok && v.pass
      lines.push(`- canary ${kind}: ${v.pass ? 'pass' : 'FAIL'} — ${v.why} ($${fmtCost(reviews.reduce((n, x) => n + (x.usage?.cost ?? 0), 0))})`)
    } finally {
      rmSync(fx.dir, { recursive: true, force: true })
    }
  }
  if (opts.health) {
    const since = new Date(Date.now() - 7 * 86_400_000).toISOString().slice(0, 10)
    const runs = gh(['api', `repos/${process.env.GITHUB_REPOSITORY || '{owner}/{repo}'}/actions/workflows/judge-review.yml/runs?created=>=${since}&per_page=100`, '--jq', '.workflow_runs[].id'], { timeout: 60_000 })
    const verdicts = []
    for (const id of (runs.ok ? runs.out.split('\n').filter(Boolean) : []).slice(0, 150)) {
      const jobs = gh(['api', `repos/${process.env.GITHUB_REPOSITORY || '{owner}/{repo}'}/actions/runs/${id}/jobs`, '--jq', '.jobs[] | select(.name | endswith("judge (review)")) | .id'])
      for (const jid of jobs.ok ? jobs.out.split('\n').filter(Boolean) : []) {
        const an = gh(['api', `repos/${process.env.GITHUB_REPOSITORY || '{owner}/{repo}'}/check-runs/${jid}/annotations`])
        if (!an.ok) continue
        verdicts.push(readReview(JSON.parse(an.out || '[]')).verdict)
      }
    }
    const h = healthOf(verdicts)
    ok = ok && h.pass
    lines.push(`- health: ${h.pass ? 'pass' : 'FAIL'} — ${h.bad} of ${h.runs} reviews in 7 days not reviewed for an unexpected reason (${(h.rate * 100).toFixed(0)}%)`)
  }
  const md = `## judge canary\n\n${lines.join('\n')}\n`
  if (opts.github && process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, md)
  process.stdout.write(md)
  return ok ? 0 : 2
}

const HELP = `judge-review — a model reads a pull request (judge (review)).

  node scripts/judge-review.mjs [--dry-run] [--pr <n>] [--json]   review this branch (bun run judge)
  node scripts/judge-review.mjs calibrate | score | ledger | canary

Needs ${KEY_ENV} for a live review; --dry-run needs no key and prints the context plan.
Exit (local review): 0 nothing to read · 2 confirmed findings to read · 1 could not run.
Under --github a review always exits 0. docs/AGENT-TOOLING.md → The judge.
`

export async function main(argv = process.argv.slice(2)) {
  const parsed = parseArgs(argv)
  if (parsed.error) return usageError(parsed.error)
  const { command, opts } = parsed
  if (opts.help || opts.h) {
    process.stdout.write(HELP)
    return 0
  }
  if (command === 'review') {
    try {
      return await cliReview(opts)
    } catch (e) {
      process.stderr.write(`judge-review: crash — ${escapeData(safeTerminal(String(e?.stack ?? e), 1200))}\n`)
      if (opts.github) {
        process.stdout.write(`::notice title=${escapeProperty(VERDICT_TITLE)}::${escapeData(verdictMessage({ status: 'not-reviewed', reason: 'crash' }, 'the reviewer crashed. Not a finding.'))}\n`)
        return 0
      }
      return 1
    }
  }
  if (command === 'fetch-prior' || command === 'score' || command === 'ledger') takeKey('__none__')
  if (command === 'fetch-prior') {
    for (const k of ['repo', 'pr', 'head', 'run-id', 'trusted', 'out']) if (!opts[k]) return usageError(`fetch-prior needs --${k}`)
    fetchPrior(opts)
    return 0
  }
  if (command === 'calibrate') return cliCalibrate(opts)
  if (command === 'score') return cliScore(opts)
  if (command === 'ledger') return cliLedger(opts)
  if (command === 'canary') return cliCanary(opts)
  return usageError(`unknown command ${command}`)
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().then(
    (code) => {
      process.exitCode = code
      // Pending timers (a deadline, an idle watch) must not hold the exit.
      setTimeout(() => process.exit(code), 50).unref?.()
    },
    (e) => {
      process.stderr.write(`judge-review: could not run — ${safeTerminal(String(e?.stack ?? e), 1200)}\n`)
      process.exitCode = process.argv.includes('--github') ? 0 : 1
    },
  )
}
