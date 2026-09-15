// Shared, pure key decisions for the chat composer — ChatComposer.svelte's
// send keymap and the suggestion menus (@ mentions, : emoji) it installs.
//
// TALA-5: pressing Enter sometimes failed to send. Two of the three gates a
// keydown passes through could swallow it:
//
//   1. The suggestion menus' onKeyDown claimed Enter/Tab unconditionally —
//      even while their candidate list was EMPTY (menu mounted by an earlier
//      query, popup hidden by an empty result set). ProseMirror consults
//      plugin handleKeyDown props BEFORE keymap plugins, so an armed-but-empty
//      menu ate the press and nothing sent: type `@` or `:` with no match,
//      press Enter, nothing happens. Now an empty menu claims nothing.
//   2. Inside an IME composition (or its trailing confirm-Enter window) Enter
//      belongs to the composition, not to send. ProseMirror already bounces
//      composition keydowns before plugins reach them on the big engines; the
//      guard here keeps the send keymap honest on any path that does reach it.
//
// Kept pure so both behaviors are testable without a DOM.

export interface EnterContext {
  /** The cursor sits inside a code block — Enter writes code there. */
  inCodeBlock: boolean
  /** An IME composition is active (or its confirm-Enter may still arrive). */
  compositionActive?: boolean
}

/** The send keymap's whole decision for Enter: send, unless the press belongs
 *  to a composition or to code-block editing. */
export function shouldSendOnEnter({ inCodeBlock, compositionActive = false }: EnterContext): boolean {
  return !compositionActive && !inCodeBlock
}

/** Which keys a suggestion menu claims, given its candidate list. A menu with
 *  nothing to select claims NOTHING — every key falls through to the editor's
 *  own handling, Enter included. A populated menu claims its selection keys
 *  (Enter/Tab pick the active row) and its navigation keys (arrows); typing,
 *  Shift-Enter, Escape, and Backspace always pass through. */
export function menuHandlesKey(key: string, items: readonly unknown[]): boolean {
  if (items.length === 0) return false
  return key === 'Enter' || key === 'Tab' || key === 'ArrowDown' || key === 'ArrowUp'
}
