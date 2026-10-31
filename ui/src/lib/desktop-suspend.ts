// The desktop shell's question, answered by the page: can it suspend me?
//
// Inside the Talaria desktop app an instance whose page has been off screen
// for a while (10 minutes by default) has its webview closed to give back its
// memory, and showing it again reloads the exact page (desktop/src-tauri/src/
// suspend.rs). Before it closes one, the shell asks the page through
// `window.__TALARIA_DESKTOP__.suspendState()`. A page without this hook (a
// Talaria older than it) is never suspended. Two things only the page knows:
//
//  1. HOLDS INPUT: something the person entered that a reload would lose.
//     - A field they changed: text typed into an input, a textarea or a rich
//       editor, a select picked, a checkbox or radio toggled. Measured
//       against what it held before they first changed it, so a field put
//       back where it was is not "held". A reload keeps the chat composer's
//       draft (lib/comms-drafts saves it as you type), so a field inside
//       `[data-desktop-draft]` does not count. Anything else — a ticket
//       description half-written, an agent's prompt mid-edit, a typed search,
//       an unsaved form's select — keeps the page alive until it is shown
//       again.
//     - Anything marked `[data-desktop-unsaved]`: state no field shows, such
//       as the pending-attachment chips over a composer (uploaded, not yet
//       sent, and not part of the draft).
//     Not seen: input inside an embedded frame. The artifact preview is a
//     sandboxed frame with its own origin, and a Google file pane is
//     Google's; neither can be read from here.
//  2. NOTIFY: would this page post an OS notification for an arrival right
//     now? The shell keeps posting them for a suspended page, with this
//     instance's own cookies (desktop/src-tauri/src/watch.rs), only if so.
//
// Installed once at boot, and only inside the shell. A browser gets nothing.
import { browserNotifyEnabled } from '@/lib/browser-notify'
import { inDesktopShell } from '@/lib/desktop-shell'

/** The hook's contract version. The shell suspends only a page at 1 or later. */
export const SUSPEND_HOOK_VERSION = 1

/** What `suspendState()` answers. */
export interface SuspendState {
  holdsInput: boolean
  notify: boolean
}

/** Marks state a reload loses that no field shows (pending attachments). */
export const UNSAVED_SELECTOR = '[data-desktop-unsaved]'

/** Inputs that hold nothing a person sets: buttons, and hidden. A picked file
 *  is not counted either: every file input in ui hands its file off on pick
 *  (it uploads, then becomes a pending-attachment chip, which is marked, or
 *  is saved at once), and what it becomes is what counts. */
const NOT_ENTERED = new Set(['submit', 'button', 'reset', 'image', 'hidden', 'file'])

/** Inputs whose state is `checked`, not `value`. */
const TOGGLES = new Set(['checkbox', 'radio'])

/** The slice of an element this module reads. Duck-typed on purpose: ui's
 *  unit tests run without a DOM. */
export interface Field {
  tagName: string
  type?: string
  value?: string
  checked?: boolean
  multiple?: boolean
  selectedOptions?: ArrayLike<{ value: string }>
  textContent: string | null
  isConnected: boolean
  disabled?: boolean
  readOnly?: boolean
  parentElement: Field | null
  closest(selector: string): Field | null
}

const EDITABLE = '[contenteditable]:not([contenteditable="false"])'

const inputType = (el: Field) => (el.type ?? 'text').toLowerCase()
const isToggle = (el: Field) => el.tagName.toUpperCase() === 'INPUT' && TOGGLES.has(inputType(el))

/** The field an event belongs to: the input, textarea or select itself, or
 *  the outermost editable host of a rich editor. */
export function fieldOf(target: EventTarget | null): Field | null {
  const el = target as unknown as Field | null
  if (!el || typeof el.closest !== 'function' || typeof el.tagName !== 'string') return null
  const tag = el.tagName.toUpperCase()
  if (tag === 'TEXTAREA' || tag === 'SELECT') return el
  if (tag === 'INPUT') return NOT_ENTERED.has(inputType(el)) ? null : el
  let host = el.closest(EDITABLE)
  if (!host) return null
  for (let up = host.parentElement?.closest(EDITABLE) ?? null; up; up = up.parentElement?.closest(EDITABLE) ?? null) {
    host = up
  }
  return host
}

function valueOf(field: Field): string {
  const tag = field.tagName.toUpperCase()
  if (isToggle(field)) return field.checked ? 'on' : 'off'
  if (tag === 'SELECT' && field.multiple && field.selectedOptions) {
    return Array.from(field.selectedOptions, (o) => o.value).join('\n')
  }
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return field.value ?? ''
  return field.textContent ?? ''
}

/** Watches what the person enters in one document. */
export class TypedInput {
  /** Each field changed, with what it held before the first change. */
  private readonly before = new Map<Field, string>()
  /** What a field held when the person last reached for it (focus or press).
   *  A select, checkbox or radio changes with no `beforeinput`, so by its
   *  `input` event the old state is gone; this is where it is read. */
  private readonly reached = new WeakMap<Field, string>()

  constructor(private readonly doc: Pick<Document, 'addEventListener' | 'querySelector'>) {
    for (const type of ['focusin', 'pointerdown']) doc.addEventListener(type, (e) => this.reach(e.target), true)
    // `beforeinput` sees the value before a typed edit. `input` (and `change`,
    // for an engine that skips `input` on a toggle) catch the rest: a select,
    // a toggle, autofill.
    doc.addEventListener('beforeinput', (e) => this.note(e.target, true), true)
    doc.addEventListener('input', (e) => this.note(e.target, false), true)
    doc.addEventListener('change', (e) => this.note(e.target, false), true)
  }

  private reach(target: EventTarget | null): void {
    const field = fieldOf(target)
    if (field && !this.before.has(field)) this.reached.set(field, valueOf(field))
  }

  private note(target: EventTarget | null, before: boolean): void {
    const field = fieldOf(target)
    if (!field || this.before.has(field)) return
    this.before.set(field, before ? valueOf(field) : this.priorOf(field))
  }

  /** What a field held before a change that came with no `beforeinput`. */
  private priorOf(field: Field): string {
    const seen = this.reached.get(field)
    if (seen !== undefined) return seen
    // Never reached for (a label clicked, a value set with no focus). A
    // checkbox was the other way; a radio that just fired was unchecked; any
    // other field held nothing we know of, so whatever it holds counts.
    if (isToggle(field)) return inputType(field) === 'checkbox' ? (field.checked ? 'off' : 'on') : 'off'
    return ''
  }

  /** True while the page holds something entered that a reload would lose:
   *  a changed field still on the page, or a `[data-desktop-unsaved]` mark.
   *  Fields that left the page are forgotten. */
  holds(): boolean {
    if (this.doc.querySelector(UNSAVED_SELECTOR)) return true
    for (const [field, was] of this.before) {
      if (!field.isConnected) {
        this.before.delete(field)
        continue
      }
      if (field.closest('[data-desktop-draft]')) continue
      if (field.disabled || field.readOnly) continue
      if (valueOf(field) !== was) return true
    }
    return false
  }
}

declare global {
  interface Window {
    __TALARIA_DESKTOP__?: { version: number; suspendState: () => SuspendState }
  }
}

/** Install the hook. Inside the desktop shell only; idempotent. */
export function installDesktopSuspend(): void {
  if (typeof window === 'undefined' || !inDesktopShell() || window.__TALARIA_DESKTOP__) return
  const typed = new TypedInput(document)
  window.__TALARIA_DESKTOP__ = {
    version: SUSPEND_HOOK_VERSION,
    suspendState: () => ({ holdsInput: typed.holds(), notify: browserNotifyEnabled() }),
  }
}
