// UNSAVED EDITS IN THE PANE, as far as the pane's host needs to know.
//
// The pane follows the agent: a file it touches for this viewer comes forward.
// Switching tabs unmounts the editor that was showing, and an editor's autosave
// timer dies with it (RichEditor, a page's HTML) — so taking focus while someone
// is mid-sentence in another file would drop their last keystrokes. The editor
// registers a question here ("is anything unsaved?"), and the host asks it
// before it moves the pane. A question rather than a flag: the answer lives in
// the editor's own state (its buffer, its timers), and is read only at the
// moment a move is decided.

const holders = new Map<string, () => boolean>()

/** Register `unsaved` for the artifact `id`; returns the unregister. A later
 *  holder for the same id replaces an earlier one, and an earlier holder's
 *  unregister never removes a later one. */
export function holdEdits(id: string, unsaved: () => boolean): () => void {
  holders.set(id, unsaved)
  return () => {
    if (holders.get(id) === unsaved) holders.delete(id)
  }
}

/** Does the editor showing artifact `id` hold edits a tab switch would lose?
 *  An editor whose answer throws is taken as holding some: the pane staying
 *  put costs a tab the person can click, and moving it could cost their
 *  keystrokes. */
export const hasUnsavedEdits = (id: string | null | undefined): boolean => {
  if (!id) return false
  try {
    return holders.get(id)?.() ?? false
  } catch {
    return true
  }
}

/** The tab the pane is showing, as far as the guard needs to know it. */
export interface ShownTab {
  id: string
  kind: string
  ref?: string | null
}

/** Would moving the pane from `active` to the tab keyed `target` drop unsaved
 *  edits? An artifact tab asks the document it shows (`ref`); a plan's own tab
 *  (kind `doc`) asks the plan's document, `planDocId`, which its editor
 *  registers under. Staying on the same tab never drops anything. */
export function moveWouldDropEdits(
  active: ShownTab | null | undefined,
  target: string,
  planDocId: string | null | undefined,
): boolean {
  if (!active || active.id === target) return false
  const shown = active.kind === 'artifact' ? active.ref : active.kind === 'doc' ? planDocId : null
  return hasUnsavedEdits(shown)
}
