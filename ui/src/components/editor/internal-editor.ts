// Shared types for <InternalEditor> (see InternalEditor.svelte).

/** Imperative surface of <InternalEditor>, grabbed with `bind:this` where
 *  `handle: InternalEditorHandle | null` — the record surface uses it to read
 *  the document on save and to stage a document a whole-form Muse filled in. */
export interface InternalEditorHandle {
  /** The editor's current markdown, or the last seeded value. */
  getMarkdown: () => string
  /** Stage a new document in the editor (unsaved). */
  setDoc: (markdown: string) => void
  /** Stage a document without marking it dirty — Cancel restoring the saved text. */
  restoreDoc: (markdown: string) => void
}
