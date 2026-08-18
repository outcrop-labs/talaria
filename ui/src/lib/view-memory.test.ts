import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import { viewMemory } from './view-memory'
import { readKnowledgeSelection, resetKnowledgeSelection, restorableKnowledgeSelection, writeKnowledgeSelection } from './knowledge-selection'
import { readArtifactsSelection, resetArtifactsSelection, restorableArtifactsSelection, writeArtifactsSelection } from './artifacts-selection'

function installStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial))
  vi.stubGlobal('window', {
    sessionStorage: {
      getItem: (k: string) => map.get(k) ?? null,
      setItem: (k: string, v: string) => void map.set(k, v),
    },
  })
  return map
}

/** A storage that refuses every write — private mode. */
function installReadOnlyStorage() {
  vi.stubGlobal('window', {
    sessionStorage: {
      getItem: () => null,
      setItem: () => {
        throw new Error('QuotaExceededError')
      },
    },
  })
}

beforeEach(() => {
  vi.unstubAllGlobals()
  resetKnowledgeSelection()
  resetArtifactsSelection()
})

// ── the shared mechanism ────────────────────────────────────────────────────

const num = (raw: unknown) => (typeof raw === 'number' ? raw : null)

test('a value written is the value read back', () => {
  installStorage()
  const m = viewMemory<number>('k', num)
  m.write(7)
  m.reset() // force a real re-read rather than the in-memory mirror
  assert.equal(m.read(), 7)
})

test('nothing stored is no memory, not a crash', () => {
  installStorage()
  assert.equal(viewMemory<number>('k', num).read(), null)
})

test('a stored shape the parser rejects degrades to no memory', () => {
  // What comes back out of storage is whatever was in there, including a shape
  // written by an older release. A selection that does not typecheck must not
  // reach a view as a malformed object.
  installStorage({ k: JSON.stringify({ not: 'a number' }) })
  assert.equal(viewMemory<number>('k', num).read(), null)
})

test('unparseable JSON degrades to no memory', () => {
  installStorage({ k: '{{{' })
  assert.equal(viewMemory<number>('k', num).read(), null)
})

test('private mode still remembers for this tab', () => {
  // The in-memory mirror is not a speed cache; it is what makes the feature
  // work where sessionStorage throws on write.
  installReadOnlyStorage()
  const m = viewMemory<number>('k', num)
  m.write(7)
  assert.equal(m.read(), 7)
})

test('the stored shape can differ from the in-memory one', () => {
  const map = installStorage()
  const m = viewMemory<number>('k', num, (v) => v * 10)
  m.write(7)
  assert.equal(map.get('k'), '70')
})

// ── Knowledge ───────────────────────────────────────────────────────────────

test('Knowledge remembers a space and the doc open in it', () => {
  installStorage()
  writeKnowledgeSelection({ spaceId: 's-1', docId: 'd-9' })
  resetKnowledgeSelection()
  assert.deepEqual(readKnowledgeSelection(), { spaceId: 's-1', docId: 'd-9' })
})

test('Knowledge remembers a space overview, which is a real place to be', () => {
  installStorage()
  writeKnowledgeSelection({ spaceId: 's-1', docId: null })
  resetKnowledgeSelection()
  assert.deepEqual(readKnowledgeSelection(), { spaceId: 's-1', docId: null })
})

test('a space that no longer exists is not restored', () => {
  // Landing someone in a space that was deleted or unshared is worse than the
  // first-space default this replaces.
  assert.equal(
    restorableKnowledgeSelection({ spaceId: 'gone', docId: 'd-9' }, { spaceIds: ['s-1', 's-2'] }),
    null,
  )
})

test('the doc is taken on trust, because its list is not loaded yet', () => {
  // Per-space and not fetched until a space is active, so there is nothing to
  // validate against at restore time. A doc that really is gone degrades to the
  // space overview via the editor's own not-found story.
  assert.deepEqual(
    restorableKnowledgeSelection({ spaceId: 's-1', docId: 'deleted' }, { spaceIds: ['s-1'] }),
    { spaceId: 's-1', docId: 'deleted' },
  )
})

test('no memory restores nothing', () => {
  assert.equal(restorableKnowledgeSelection(null, { spaceIds: ['s-1'] }), null)
})

// ── Artifacts ───────────────────────────────────────────────────────────────

test('Artifacts remembers place, folder and open file together', () => {
  installStorage()
  writeArtifactsSelection({ place: 'shared', folderId: 'f-1', activeId: 'a-1' })
  resetArtifactsSelection()
  assert.deepEqual(readArtifactsSelection(), { place: 'shared', folderId: 'f-1', activeId: 'a-1' })
})

test('a place that is no longer a place drops the whole memory', () => {
  // Silently relocating someone to `my` would be a worse answer than starting
  // fresh, because it looks like the memory worked.
  installStorage({ 'talaria:artifacts-selection': JSON.stringify({ place: 'atlantis', folderId: 'f-1' }) })
  assert.equal(readArtifactsSelection(), null)
})

test('a deleted folder does not cost you the place', () => {
  // Each part is independently useful; discarding the lot sends someone to the
  // root of `my` when the place they were in is perfectly fine.
  assert.deepEqual(
    restorableArtifactsSelection(
      { place: 'workspace', folderId: 'gone', activeId: 'a-1' },
      { folderIds: ['f-1'], artifactIds: ['a-1'] },
    ),
    // The file goes with the folder: it would otherwise render detached from
    // the browser around it.
    { place: 'workspace', folderId: null, activeId: null },
  )
})

test('a deleted file does not cost you the folder', () => {
  assert.deepEqual(
    restorableArtifactsSelection(
      { place: 'my', folderId: 'f-1', activeId: 'gone' },
      { folderIds: ['f-1'], artifactIds: ['a-1'] },
    ),
    { place: 'my', folderId: 'f-1', activeId: null },
  )
})

test('a roster that has not loaded takes its part on trust', () => {
  // Discarding a good folder for a list that merely has not arrived yet is the
  // same race in a new place.
  assert.deepEqual(
    restorableArtifactsSelection(
      { place: 'my', folderId: 'f-1', activeId: 'a-1' },
      { folderIds: null, artifactIds: null },
    ),
    { place: 'my', folderId: 'f-1', activeId: 'a-1' },
  )
})
