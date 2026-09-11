// THE FILE CLIPBOARD — cut and copy for the Files view.
//
// A clipboard is not a selection: it survives navigation (that is its entire
// point — you cut HERE and paste THERE), it has a MODE (cut moves on paste,
// copy duplicates), and it is never more than one batch. Module-scope $state
// behind accessor functions, the confirm/toast idiom — cross-module reactivity
// requires the functions, and mutation from outside is impossible.
//
// The key grammar is the browser's own `${type}:${id}`, so a clipboard batch
// and a selection use the same vocabulary; `fromFolderId` remembers where a
// cut came from so paste-in-place can no-op (a copy pasted in place is a
// duplicate — Drive semantics, and the useful one).

export type ClipboardMode = 'cut' | 'copy'

export interface FilesClipboard {
  mode: ClipboardMode
  keys: string[]
  fromFolderId: string | null
}

const store = $state<{ current: FilesClipboard | null }>({ current: null })

export function cutClipboard(keys: string[], fromFolderId: string | null): void {
  if (!keys.length) return
  store.current = { mode: 'cut', keys: [...keys], fromFolderId }
}

export function copyClipboard(keys: string[], fromFolderId: string | null): void {
  if (!keys.length) return
  store.current = { mode: 'copy', keys: [...keys], fromFolderId }
}

export function clearClipboard(): void {
  store.current = null
}

export function clipboard(): FilesClipboard | null {
  return store.current
}

/** Is this row on the clipboard (either mode)? Drives the cut dimming. */
export function clipboardHas(key: string): boolean {
  return store.current?.keys.includes(key) ?? false
}

/** Is this row CUT (on the clipboard in cut mode)? The dimming affordance —
 *  a copied row looks normal; a cut row is spoken for. */
export function isCut(key: string): boolean {
  return store.current?.mode === 'cut' && store.current.keys.includes(key)
}

/** Drop keys whose rows no longer exist — deleting a cut item must not leave
 *  it armed for the next paste. Called by the browser when the rosters
 *  change, exactly like its selection-pruning effect. */
export function pruneClipboard(liveKeys: Set<string>): void {
  const c = store.current
  if (!c) return
  const keys = c.keys.filter((k) => liveKeys.has(k))
  if (keys.length === c.keys.length) return
  store.current = keys.length ? { ...c, keys } : null
}
