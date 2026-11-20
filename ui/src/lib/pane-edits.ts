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

/** Does the editor showing artifact `id` hold edits a tab switch would lose? */
export const hasUnsavedEdits = (id: string | null | undefined): boolean => {
  if (!id) return false
  try {
    return holders.get(id)?.() ?? false
  } catch {
    return false
  }
}
