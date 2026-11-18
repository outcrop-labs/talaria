// judge-review-wire — the one contract between the reviewer and its readers.
//
// scripts/judge-review.mjs WRITES it: a verdict notice and at most three
// line annotations on its `judge (review)` job. scripts/hooks/pr-watch.mjs and
// scripts/hooks/remote-gate.mjs READ it back from GitHub's check-run
// annotations, and pr-watch reads the `judge-review answer` comments beside it.
// Nothing else crosses between them, so a change to a title, a key or the id
// rule lands here once and every side moves with it.
//
// WHY ANNOTATIONS. They are the only output of a job a reader can fetch with
// a read token and no artifact download: GET check-runs/<job>/annotations.
// The model's text never reaches a title: a title is built from our own values
// (the id, the severity, the category, the mark), and the verdict's values are
// each checked against a pattern on the way out and on the way back in.
//
// THE ID. `jr-` + 8 hex of sha256(file, the head's normalised line at the
// anchor, how many identical lines come before it). It depends on the head's
// text only — never on how many lines the model quoted, on its category or on
// its wording — so two samples that flag the same line agree, and an answer
// given on one head still names the finding on the next while that line holds.
//
// Stdlib only, like everything under scripts/.

import { createHash } from 'node:crypto'

export const VERDICT_TITLE = 'judge-review verdict'
export const REVIEW_CHECK = 'judge (review)'
export const OFFLOAD_REVIEW_CHECK = 'offload / judge (review)'

export const SEVERITIES = ['blocker', 'major', 'minor']
export const MARKS = ['gate', 'would-gate']

/** A Tier A annotation's title. No `:` or `,`: a title is a workflow-command
 *  property, and both would have to be escaped. */
export function findingTitle({ id, severity, category, mark }) {
  const tail = mark === 'gate' ? ' gate' : mark === 'would-gate' ? ' would-gate' : ''
  return `judge-review ${id} ${severity} ${category}${tail}`
}

/** Only Tier A is annotated, and Tier A is never minor. */
export const FINDING_TITLE_RE = /^judge-review (jr-[0-9a-f]{8}) (blocker|major) ([a-z-]+)( gate| would-gate)?$/

const SLUG = /^[A-Za-z0-9._/-]{1,64}$/

/** The verdict's keys, in the order they are written, and what each value may be. */
export const VERDICT_KEYS = [
  ['status', /^(reviewed|not-reviewed|skipped)$/],
  ['head', /^[0-9a-f]{12}$/],
  ['base', /^([0-9a-f]{12}|na)$/],
  ['target', SLUG],
  ['listed', /^\d{1,4}$/],
  ['more', /^\d{1,4}$/],
  ['gating', /^\d{1,4}$/],
  ['would', /^\d{1,4}$/],
  ['gate', /^(off|report|blockers|na)$/],
  ['cost', /^(\d+\.\d{4}|na)$/],
  ['model', SLUG],
  ['provider', SLUG],
  ['reason', SLUG],
  ['reused', /^(\d{1,20}|none)$/],
  ['partial', /^(yes|no)$/],
]
const VERDICT_KEY_MAP = new Map(VERDICT_KEYS)

/** Our own values only: provider, model, target and reason pass through here. */
export function slug(s) {
  return String(s ?? '').replace(/[^A-Za-z0-9._/-]/g, '-').slice(0, 64) || 'na'
}

/** `k=v` pairs in key order, then ` — ` and our own sentence. A value that
 *  does not match its pattern is left out rather than written wrong. */
export function verdictMessage(v, human = '') {
  const parts = []
  for (const [key, re] of VERDICT_KEYS) {
    if (v[key] === undefined || v[key] === null) continue
    const value = String(v[key])
    if (re.test(value)) parts.push(`${key}=${value}`)
  }
  const sentence = String(human).replace(/[\r\n]+/g, ' ').trim()
  return sentence ? `${parts.join(' ')} — ${sentence}` : parts.join(' ')
}

/** The verdict back, or null: only known keys, each at most once, each value
 *  matching its pattern, and `status` present. */
export function parseVerdict(message) {
  const text = String(message ?? '')
  const cut = text.indexOf(' — ')
  const head = (cut === -1 ? text : text.slice(0, cut)).trim()
  const human = cut === -1 ? '' : text.slice(cut + 3).trim()
  if (!head) return null
  const out = {}
  for (const pair of head.split(/ +/)) {
    const eq = pair.indexOf('=')
    if (eq <= 0) return null
    const key = pair.slice(0, eq)
    const value = pair.slice(eq + 1)
    const re = VERDICT_KEY_MAP.get(key)
    if (!re || Object.hasOwn(out, key) || !re.test(value)) return null
    out[key] = value
  }
  if (!out.status) return null
  return { ...out, human }
}

/** One line as the id and the anchor compare it: whitespace collapsed, trimmed. */
export function norm(line) {
  return String(line ?? '').replace(/\s+/g, ' ').trim()
}

/** The finding's id, from the head's text at the anchored line (1-based). */
export function findingId(file, headLines, anchorLine) {
  const n = norm(headLines[anchorLine - 1])
  let k = 0
  for (let i = 0; i < anchorLine - 1 && i < headLines.length; i++) if (norm(headLines[i]) === n) k++
  return `jr-${createHash('sha256').update(`${file}\0${n}\0${k}`).digest('hex').slice(0, 8)}`
}

export const ANSWER_RE = /^\s*judge-review answer (jr-[0-9a-f]{8})\s*[:—-]\s*(.+?)\s*$/gm
export const MIN_ANSWER = 40
const TRUSTED_ASSOCIATIONS = new Set(['OWNER', 'MEMBER', 'COLLABORATOR'])

/** Map id → {by, reason, url, at}: answers from the pull request's author
 *  (a bot author included: the Talaria App answers as itself) or from a
 *  person who owns, is a member of, or collaborates on the repository. Never
 *  github-actions[bot]. A reason under MIN_ANSWER characters is not an answer.
 *  The latest answer to an id wins. `comments` may be pages (`--slurp`). */
export function parseAnswers(comments, prAuthorLogin) {
  const rows = (Array.isArray(comments) ? comments : []).flat(2).filter((c) => c && typeof c === 'object')
  rows.sort((a, b) => Date.parse(a.created_at || 0) - Date.parse(b.created_at || 0))
  const out = new Map()
  for (const c of rows) {
    const login = String(c.user?.login ?? '')
    if (!login || login === 'github-actions[bot]') continue
    const isAuthor = prAuthorLogin && login === prAuthorLogin
    const isPerson = c.user?.type === 'User' && TRUSTED_ASSOCIATIONS.has(String(c.author_association ?? ''))
    if (!isAuthor && !isPerson) continue
    for (const m of String(c.body ?? '').matchAll(ANSWER_RE)) {
      const reason = m[2].trim()
      if (reason.length < MIN_ANSWER) continue
      out.set(m[1], { by: login, reason, url: c.html_url ?? '', at: c.created_at ?? '' })
    }
  }
  return out
}

/** The review a job's annotations carry: the verdict (a reviewed one wins
 *  over any other) and the Tier A findings. Rows that match neither are
 *  ignored — a job's annotations also carry the runner's own. */
export function readReview(annotations) {
  const rows = (Array.isArray(annotations) ? annotations : []).flat(2).filter((a) => a && typeof a === 'object')
  let verdict = null
  const findings = []
  for (const a of rows) {
    const title = String(a.title ?? '')
    if (title === VERDICT_TITLE) {
      const v = parseVerdict(a.message)
      if (v && (!verdict || (v.status === 'reviewed' && verdict.status !== 'reviewed'))) verdict = v
      continue
    }
    const m = FINDING_TITLE_RE.exec(title)
    if (!m) continue
    findings.push({
      id: m[1],
      severity: m[2],
      category: m[3],
      mark: m[4] ? m[4].trim() : null,
      path: String(a.path ?? ''),
      line: Number(a.start_line ?? a.line ?? 0) || 0,
      endLine: Number(a.end_line ?? a.start_line ?? 0) || 0,
      message: String(a.message ?? ''),
    })
  }
  return { verdict, findings }
}

/** The findings that hold an agent's done: marked `gate` and unanswered. */
export function holding(review, answers) {
  const a = answers instanceof Map ? answers : new Map()
  return (review?.findings ?? []).filter((f) => f.mark === 'gate' && !a.has(f.id))
}

/** A workflow command's message. */
export function escapeData(s) {
  return String(s ?? '').replace(/%/g, '%25').replace(/\r/g, '%0D').replace(/\n/g, '%0A')
}

/** A workflow command's property value (file=, title=). */
export function escapeProperty(s) {
  return escapeData(s).replace(/:/g, '%3A').replace(/,/g, '%2C')
}

// eslint-disable-next-line no-control-regex
const ANSI_RE = /\u001b(?:\[[0-9;?]*[ -/]*[@-~]|\][^\u0007\u001b]*(?:\u0007|\u001b\\)?|[@-Z\\-_])/g
// eslint-disable-next-line no-control-regex
const CONTROL_RE = /[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g
export const BIDI_ZW_RE = /[‪-‮⁦-⁩​-‏﻿]/g

/** Text safe for a terminal: no escape sequences, no C0/C1 control
 *  characters but the newline, no bidi overrides or zero-width characters,
 *  and at most `cap` characters. */
export function safeTerminal(s, cap = 2000) {
  const out = String(s ?? '').replace(ANSI_RE, '').replace(CONTROL_RE, '').replace(BIDI_ZW_RE, '')
  return out.length > cap ? `${out.slice(0, Math.max(0, cap - 1))}…` : out
}
