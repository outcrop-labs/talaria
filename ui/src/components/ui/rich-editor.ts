// Shared types for <RichEditor> (see RichEditor.svelte).

/** Imperative surface of <RichEditor>. In React this was a forwardRef handle;
 *  in Svelte the component instance itself satisfies it — grab it with
 *  `bind:this={handle}` where `handle: RichEditorHandle | null`. */
export interface RichEditorHandle {
  getMarkdown: () => string
  clear: () => void
  /** Selected text (plain), '' when empty — context menus and inline Muse. */
  getSelectionText: () => string
  /** Replace the current selection with markdown/text. */
  replaceSelection: (content: string) => void
  /** Toggle an inline mark on the selection (context-menu formatting). */
  toggleMark: (mark: 'bold' | 'italic' | 'strike' | 'code') => void
  /** True when the caret sits inside a table (gates table menu items). */
  isInTable: () => boolean
  /** Table structure ops for context menus. */
  tableCommand: (
    cmd: 'addRowBefore' | 'addRowAfter' | 'addColumnBefore' | 'addColumnAfter' | 'deleteRow' | 'deleteColumn' | 'deleteTable' | 'insertTable',
  ) => void
}

/** Optional cross-reference search: given a query, return docs to link to.
 *  When provided, the toolbar gains a "link to doc" button with fuzzy search. */
export type DocSearchFn = (query: string) => Promise<Array<{ id: string; title: string; icon?: string | null; href: string }>>
