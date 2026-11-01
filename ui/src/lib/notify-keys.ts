// The notification center's keyboard (the /notifications page), Gmail/Linear
// style. PURE: a key event's facts go in, an action comes out, and the
// focus/selection arithmetic is a function of the visible rows — so every key
// is pinned by a test, and the page only wires the DOM.
//
//   j / ↓      next row            k / ↑      previous row
//   Enter / o  open                e          Done (the selection, else the row)
//   u          read ↔ unread       Shift+E    Done the selection, else every visible row
//   x          select / unselect   ?          this help
//   Esc        clear the selection, close the help

export type KeyAction =
  | 'next'
  | 'prev'
  | 'open'
  | 'done'
  | 'doneAll'
  | 'toggleRead'
  | 'select'
  | 'help'
  | 'escape'

export interface KeyInput {
  key: string
  shiftKey?: boolean
  ctrlKey?: boolean
  metaKey?: boolean
  altKey?: boolean
  /** The event's target takes text (an input, a select, contenteditable). */
  editable?: boolean
}

/** The action a keydown means, or null to leave it to the browser. A key
 *  typed into a field is the field's; a chord with Ctrl/Cmd/Alt is the
 *  browser's or the OS's (Ctrl+E focuses the address bar in Firefox). */
export function keyAction(e: KeyInput): KeyAction | null {
  if (e.editable) return null
  if (e.ctrlKey || e.metaKey || e.altKey) return null
  switch (e.key) {
    case 'j':
    case 'ArrowDown':
      return 'next'
    case 'k':
    case 'ArrowUp':
      return 'prev'
    case 'Enter':
    case 'o':
      return 'open'
    case 'e':
      return e.shiftKey ? 'doneAll' : 'done'
    case 'E':
      return 'doneAll'
    case 'u':
      return 'toggleRead'
    case 'x':
      return 'select'
    case '?':
      return 'help'
    case 'Escape':
      return 'escape'
    default:
      return null
  }
}

/** The help sheet, in the order the keys are learned. */
export const KEY_HELP: ReadonlyArray<{ keys: string[]; what: string }> = [
  { keys: ['j', '↓'], what: 'Next notification' },
  { keys: ['k', '↑'], what: 'Previous notification' },
  { keys: ['Enter', 'o'], what: 'Open it' },
  { keys: ['e'], what: 'Done (the selected ones, else this one)' },
  { keys: ['u'], what: 'Mark read or unread' },
  { keys: ['x'], what: 'Select or unselect' },
  { keys: ['Shift', 'E'], what: 'Done: the selected ones, else everything shown' },
  { keys: ['?'], what: 'Show these shortcuts' },
  { keys: ['Esc'], what: 'Clear the selection' },
]

/** The focused index after a move: the first row on the first `j`, clamped
 *  at both ends (no wrap — a wrap at the end of a long list reads as a jump
 *  to the top). -1 = nothing focused. */
export function moveFocus(index: number, count: number, action: 'next' | 'prev'): number {
  if (count <= 0) return -1
  if (index < 0 || index >= count) return action === 'next' ? 0 : count - 1
  return action === 'next' ? Math.min(index + 1, count - 1) : Math.max(index - 1, 0)
}

/** Keep the focus on the same row after the list changed under it (a Done
 *  removed rows, a page loaded): the same id if it is still there, else the
 *  row now at its old place, else the last row. */
export function refocus(prevId: string | null, prevIndex: number, ids: readonly string[]): number {
  if (ids.length === 0) return -1
  if (prevId) {
    const i = ids.indexOf(prevId)
    if (i >= 0) return i
  }
  if (prevIndex < 0) return -1
  return Math.min(prevIndex, ids.length - 1)
}

export function toggleSelected(sel: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(sel)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  return next
}

/** The rows an action applies to: the selection when there is one, else the
 *  focused row — and for `doneAll`, else every visible row. A selection that
 *  scrolled out of the view still counts only where it is visible. */
export function targetsOf(
  action: 'done' | 'doneAll' | 'toggleRead',
  ctx: { focused: string | null; selection: ReadonlySet<string>; visible: readonly string[] },
): string[] {
  const selected = ctx.visible.filter((id) => ctx.selection.has(id))
  if (selected.length) return selected
  if (action === 'doneAll') return [...ctx.visible]
  return ctx.focused ? [ctx.focused] : []
}

/** What `u` does to a set of rows: read them when any is unread, else make
 *  them all unread again. */
export function readToggle(rows: ReadonlyArray<{ readAt: string | null }>): 'read' | 'unread' {
  return rows.some((r) => !r.readAt) ? 'read' : 'unread'
}

/** Whether a keydown target takes text — the DOM half of `editable`. */
export function isEditableTarget(t: EventTarget | null): boolean {
  if (!t || typeof (t as HTMLElement).closest !== 'function') return false
  const el = t as HTMLElement
  return !!el.closest('input, textarea, select, [contenteditable=""], [contenteditable="true"]')
}
