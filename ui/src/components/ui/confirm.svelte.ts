// In-app replacements for the native window.confirm / .alert / .prompt dialogs.
//
// These are imperative async functions so they drop into the existing
// `if (!confirm(...)) return` call sites unchanged — just `await` them:
//
//   if (!(await confirm({ message: 'Delete this?', danger: true }))) return
//
// A single <ConfirmHost /> mounted at the app root renders the actual Modal
// off the reactive `dialog` state below. Because dialogs are only ever
// triggered by user interaction (well after mount), the host is always there
// by the time these run.

type Kind = 'confirm' | 'alert' | 'prompt'

export interface DialogSpec {
  kind: Kind
  title?: string
  /** Body text. (React allowed nodes here; every call site passes strings.) */
  message?: string
  confirmLabel?: string
  cancelLabel?: string
  /** Style the confirm action as destructive — orange outline, never orange
   *  fill (Mercury spec §8 DANGER ZONE). */
  danger?: boolean
  /** prompt only. */
  placeholder?: string
  /** prompt only — initial input value. */
  defaultValue?: string
}

type Resolve = (value: boolean | string | null) => void

interface ActiveDialog {
  spec: DialogSpec
  resolve: Resolve
}

/** Reactive slot ConfirmHost renders from. One dialog at a time. */
export const dialog = $state<{ active: ActiveDialog | null }>({ active: null })

function request(spec: DialogSpec): Promise<boolean | string | null> {
  if (typeof window === 'undefined') return Promise.resolve(spec.kind === 'alert' ? null : false)
  return new Promise((resolve) => {
    dialog.active = { spec, resolve }
  })
}

export type ConfirmOptions = Omit<DialogSpec, 'kind'>

/** Ask the user to confirm an action. Resolves true if confirmed, false otherwise. */
export function confirm(opts: ConfirmOptions): Promise<boolean> {
  return request({ ...opts, kind: 'confirm' }).then(Boolean)
}

/** Show an acknowledgement / error notice with a single dismiss button. */
export function alert(opts: ConfirmOptions): Promise<void> {
  return request({ ...opts, kind: 'alert' }).then(() => undefined)
}

/** Ask the user for a line of text. Resolves the entered string, or null if cancelled. */
export function prompt(opts: ConfirmOptions): Promise<string | null> {
  return request({ ...opts, kind: 'prompt' }).then((v) => (typeof v === 'string' ? v : null))
}
