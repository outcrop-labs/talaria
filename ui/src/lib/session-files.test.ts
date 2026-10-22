import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import {
  appendPin,
  changedFiles,
  fileGlyph,
  fileTitle,
  fileTypeLabel,
  focusTarget,
  googleMime,
  googleOpenHref,
  hiddenFiles,
  hideFile,
  initialFocus,
  newestTouched,
  parseSessionFiles,
  resurfaced,
  unhideFile,
  visibleFiles,
  type SessionFile,
} from './session-files'

const HIDDEN_KEY = 'talaria:workspace-hidden:v1'

const art = (id: string, lastTouchedAt: string | null, extra: Partial<SessionFile> = {}): SessionFile => ({
  key: `artifact:${id}`,
  kind: 'artifact',
  id,
  title: `Doc ${id}`,
  artifactKind: 'doc',
  source: lastTouchedAt ? 'touched' : 'pinned',
  lastTouchedAt,
  ...extra,
})
const goog = (id: string, lastTouchedAt: string | null, googleType: SessionFile['googleType'] = 'document'): SessionFile => ({
  key: `google:${id}`,
  kind: 'google',
  id,
  title: null,
  googleType,
  source: lastTouchedAt ? 'touched' : 'pinned',
  lastTouchedAt,
})

const T1 = '2026-10-21T10:00:00Z'
const T2 = '2026-10-21T10:05:00Z'
const T3 = '2026-10-21T10:09:00Z'

function installLocalStorage() {
  const map = new Map<string, string>()
  vi.stubGlobal('window', {
    localStorage: {
      getItem: (k: string) => (map.has(k) ? map.get(k)! : null),
      setItem: (k: string, v: string) => void map.set(k, v),
      removeItem: (k: string) => void map.delete(k),
    },
  })
  return map
}

beforeEach(() => vi.unstubAllGlobals())

// ── the focus rule ──────────────────────────────────────────────────────────

test('a session opens on the remembered tab when the session still lists it', () => {
  const files = [art('a', T1), art('b', T3), art('c', T2)]
  assert.equal(initialFocus(files, 'artifact:c'), 'artifact:c')
})

test('without a usable memory it opens on the newest touch, then on the first file', () => {
  const files = [art('a', T1), art('b', T3), art('c', T2)]
  assert.equal(initialFocus(files, null), 'artifact:b')
  assert.equal(initialFocus(files, 'artifact:gone'), 'artifact:b', 'a remembered file the session no longer lists')
  // Pins and the legacy living document carry no touch time: first file wins.
  assert.equal(initialFocus([art('p', null), goog('g', null)], null), 'artifact:p')
  assert.equal(initialFocus([], 'artifact:a'), null)
})

test('newest touch ties go to the later row, and untouched rows never win', () => {
  const a = art('a', T2)
  const b = art('b', T2)
  assert.equal(newestTouched([a, b]), b)
  assert.equal(newestTouched([art('x', null)]), null)
})

// ── what a refetch brings ───────────────────────────────────────────────────

test('the first read brings nothing: it opens on the focus rule instead', () => {
  assert.deepEqual(changedFiles(null, [art('a', T1)]), [])
})

test('a new file and a re-touched file are changes; an untouched one is not', () => {
  const prev = [art('a', T1), art('b', T2), goog('g', null)]
  const next = [art('a', T1), art('b', T3), goog('g', null), art('c', T2)]
  assert.deepEqual(
    changedFiles(prev, next).map((f) => f.key),
    ['artifact:b', 'artifact:c'],
  )
})

test('a touch time that moved backwards, or vanished, is not a re-touch', () => {
  const prev = [art('a', T2)]
  assert.deepEqual(changedFiles(prev, [art('a', T1)]), [])
  assert.deepEqual(changedFiles(prev, [art('a', null)]), [])
  // A pin the agent then touches gains a time: that IS a touch.
  assert.deepEqual(changedFiles([art('p', null)], [art('p', T1)]).map((f) => f.key), ['artifact:p'])
})

test('of several changes, the newest touch takes focus; a hand-opened pin only when nothing was touched', () => {
  const touched = art('t', T2)
  const newer = art('n', T3)
  const pin = art('p', null)
  assert.equal(focusTarget([touched, pin, newer]), newer)
  assert.equal(focusTarget([pin]), pin)
  assert.equal(focusTarget([]), null)
})

// ── closed tabs ─────────────────────────────────────────────────────────────

test('a closed file stays hidden until a newer message touches it', () => {
  installLocalStorage()
  const s = 'sess-hidden-1'
  const a = art('a', T1)
  const g = goog('g', null)
  hideFile(s, a)
  hideFile(s, g)
  const hidden = hiddenFiles(s)
  assert.deepEqual(visibleFiles([a, g, art('b', T2)], hidden).map((f) => f.key), ['artifact:b'])

  // Nothing moved: both stay hidden.
  assert.deepEqual(resurfaced(hidden, [a, g]), [])
  // The agent touches both again: each comes back, the pin by GAINING a time.
  assert.deepEqual(resurfaced(hidden, [art('a', T2), goog('g', T3)]), ['artifact:a', 'google:g'])

  unhideFile(s, 'artifact:a')
  assert.deepEqual(Object.keys(hiddenFiles(s)), ['google:g'])
})

test('the hidden memory is one bounded key, and a malformed value reads as none', () => {
  const map = installLocalStorage()
  for (let i = 0; i < 45; i += 1) hideFile(`sess-many-${i}`, art('a', T1))
  const stored = JSON.parse(map.get(HIDDEN_KEY) ?? '{}') as Record<string, unknown>
  assert.equal(map.size, 1)
  assert.equal(Object.keys(stored).length, 40, 'the stalest sessions fall off')
  assert.ok(!('sess-many-0' in stored))
  assert.ok('sess-many-44' in stored)

  for (const raw of ['not json', '[]', '"x"', 'null', '{"s":[1]}']) {
    map.set(HIDDEN_KEY, raw)
    assert.deepEqual(hiddenFiles('s'), {}, raw)
  }
})

test('without localStorage nothing is hidden and nothing throws', () => {
  assert.equal(typeof globalThis.window, 'undefined')
  hideFile('sess-nostore', art('a', T1))
  assert.deepEqual(hiddenFiles('sess-nostore'), {})
})

// ── names and types ─────────────────────────────────────────────────────────

test('every type has a label and an untitled name', () => {
  assert.equal(fileTypeLabel(art('a', null)), 'Document')
  assert.equal(fileTypeLabel(art('a', null, { artifactKind: 'sheet' })), 'Sheet')
  assert.equal(fileTypeLabel(art('a', null, { artifactKind: 'microsite' })), 'Page')
  assert.equal(fileTypeLabel(art('a', null, { artifactKind: 'run-transcript' })), 'File')
  assert.equal(fileTypeLabel(goog('g', null, 'document')), 'Google Doc')
  assert.equal(fileTypeLabel(goog('g', null, 'spreadsheet')), 'Google Sheet')
  assert.equal(fileTypeLabel(goog('g', null, 'presentation')), 'Google Slides')
  assert.equal(fileTypeLabel(goog('g', null, 'drive')), 'Drive file')

  assert.equal(fileTitle(art('a', null, { title: '  ' })), 'Untitled document')
  assert.equal(fileTitle(art('a', null, { title: null, artifactKind: 'sheet' })), 'Untitled sheet')
  assert.equal(fileTitle(goog('g', null, 'spreadsheet')), 'Google Sheet')
  assert.equal(fileTitle(art('a', null, { title: 'Q3 plan' })), 'Q3 plan')
  assert.equal(fileGlyph({ kind: 'google' }), 'google-drive')
})

test('a Google file opens at its canonical URL, and only a Google https one', () => {
  assert.equal(googleMime(goog('g', null, 'spreadsheet')), 'application/vnd.google-apps.spreadsheet')
  assert.equal(googleMime(goog('g', null, 'drive')), null)
  const g = goog('abc_123', null, 'document')
  assert.equal(googleOpenHref({ ...g, url: 'https://docs.google.com/document/d/abc_123/edit' }), 'https://docs.google.com/document/d/abc_123/edit')
  assert.equal(googleOpenHref({ ...g, url: 'javascript:alert(1)' }), 'https://docs.google.com/document/d/abc_123/edit')
  assert.equal(googleOpenHref({ ...g, url: 'https://evil.example/document/d/abc_123' }), 'https://docs.google.com/document/d/abc_123/edit')
  assert.equal(googleOpenHref(goog('f1', null, 'drive')), 'https://drive.google.com/file/d/f1/view')
})

// ── the wire ────────────────────────────────────────────────────────────────

test('the answer parses, and rows this build cannot render drop out', () => {
  const out = parseSessionFiles({
    kind: 'work',
    files: [
      { key: 'artifact:1', kind: 'artifact', id: '1', title: 'One', artifactKind: 'doc', source: 'touched', lastTouchedAt: T1 },
      { key: 'google:g', kind: 'google', id: 'g', title: null, googleType: 'presentation', url: 'https://docs.google.com/presentation/d/g', source: 'pinned', lastTouchedAt: null },
      { key: 'x', kind: 'kb', id: 'k', source: 'touched' },
      { key: 'artifact:2', kind: 'artifact', id: '', source: 'touched' },
      null,
      'nope',
    ],
  })
  assert.equal(out.kind, 'work')
  assert.deepEqual(out.files.map((f) => f.key), ['artifact:1', 'google:g'])
  assert.equal(out.files[1]?.googleType, 'presentation')
  assert.equal(out.files[1]?.source, 'pinned')
  assert.deepEqual(parseSessionFiles(null), { kind: 'work', files: [] })
})

test('opening a file by hand appends it to the pins, never twice', () => {
  const a = { kind: 'artifact' as const, id: 'a' }
  const g = { kind: 'google' as const, id: 'g', mime: 'application/vnd.google-apps.document' }
  assert.deepEqual(appendPin([a], g), [a, g])
  assert.deepEqual(appendPin([a, g], a), [g, a], 'a re-pin moves to the end rather than duplicating')
})
