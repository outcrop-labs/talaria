// The desktop shell's question, answered by the page: can it suspend me?
//
// Inside the Talaria desktop app an instance whose page has been off screen
// for a while (10 minutes by default) has its webview closed to give back its
// memory, and showing it again reloads the exact page (desktop/src-tauri/src/
// suspend.rs). Before it closes one, the shell asks the page through
// `window.__TALARIA_DESKTOP__.suspendState()`. A page without this hook (a
// Talaria older than it) is never suspended. Two things only the page knows:
//
//  1. HOLDS INPUT: text the person typed that a reload would lose. A reload
//     keeps the chat composer's draft (lib/comms-drafts saves it as you type),
//     so a field inside `[data-desktop-draft]` does not count. Anything else
//     — a ticket description half-written, an agent's prompt mid-edit, a
//     typed search — keeps the page alive until it is shown again. Measured
//     against what the field held before the person first typed in it, so a
//     field typed back to where it was is not "held".
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

/** Text inputs a person types into. Checkboxes, radios, ranges, colours and
 *  files are not typed input. */
const TYPED = new Set(['text', 'search', 'email', 'url', 'tel', 'password', 'number'])

/** The slice of an element this module reads. Duck-typed on purpose: ui's
 *  unit tests run without a DOM. */
export interface Field {
  tagName: string
  type?: string
  value?: string
  textContent: string | null
  isConnected: boolean
  disabled?: boolean
  readOnly?: boolean
  parentElement: Field | null
  closest(selector: string): Field | null
}

const EDITABLE = '[contenteditable]:not([contenteditable="false"])'

/** The field an input event belongs to: the input or textarea itself, or the
 *  outermost editable host of a rich editor. */
export function fieldOf(target: EventTarget | null): Field | null {
  const el = target as unknown as Field | null
  if (!el || typeof el.closest !== 'function' || typeof el.tagName !== 'string') return null
  const tag = el.tagName.toUpperCase()
  if (tag === 'TEXTAREA') return el
  if (tag === 'INPUT') return TYPED.has((el.type ?? 'text').toLowerCase()) ? el : null
  let host = el.closest(EDITABLE)
  if (!host) return null
  for (let up = host.parentElement?.closest(EDITABLE) ?? null; up; up = up.parentElement?.closest(EDITABLE) ?? null) {
    host = up
  }
  return host
}

function valueOf(field: Field): string {
  const tag = field.tagName.toUpperCase()
  if (tag === 'INPUT' || tag === 'TEXTAREA') return field.value ?? ''
  return field.textContent ?? ''
}

/** Watches what the person types in one document. */
export class TypedInput {
  /** Each field typed in, with what it held before the first keystroke. */
  private readonly before = new Map<Field, string>()

  constructor(doc: Pick<Document, 'addEventListener'>) {
    // `beforeinput` sees the value before the edit. `input` alone catches
    // what has no beforeinput (autofill): the field held nothing we know of.
    doc.addEventListener('beforeinput', (e) => this.note(e.target, true), true)
    doc.addEventListener('input', (e) => this.note(e.target, false), true)
  }

  private note(target: EventTarget | null, before: boolean): void {
    const field = fieldOf(target)
    if (!field || this.before.has(field)) return
    this.before.set(field, before ? valueOf(field) : '')
  }

  /** True while some field still on the page holds typed text a reload would
   *  lose. Fields that left the page are forgotten. */
  holds(): boolean {
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
