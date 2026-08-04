import { useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { Modal } from './modal'
import { Button } from './button'
import { Input } from './input'

// In-app replacements for the native window.confirm / .alert / .prompt dialogs.
//
// These are imperative async functions so they drop into the existing
// `if (!confirm(...)) return` call sites unchanged — just `await` them:
//
//   if (!(await confirm({ message: 'Delete this?', danger: true }))) return
//
// A single <ConfirmHost /> mounted at the app root renders the actual Modal and
// wires up the module-level `dispatch`. Because dialogs are only ever triggered
// by user interaction (well after hydration), the host is always mounted by the
// time these run.

type Kind = 'confirm' | 'alert' | 'prompt'

interface DialogSpec {
  kind: Kind
  title?: ReactNode
  /** Body text or nodes. */
  message?: ReactNode
  confirmLabel?: string
  cancelLabel?: string
  /** Style the confirm action as destructive — orange outline, never orange
   *  fill (Gentle dew spec §8 DANGER ZONE). */
  danger?: boolean
  /** prompt only. */
  placeholder?: string
  /** prompt only — initial input value. */
  defaultValue?: string
}

type Resolve = (value: boolean | string | null) => void

let dispatch: ((spec: DialogSpec, resolve: Resolve) => void) | null = null

function request(spec: DialogSpec): Promise<boolean | string | null> {
  if (!dispatch) {
    // Host not mounted (SSR, or a dialog fired before hydration). Degrade to the
    // native dialog rather than silently dropping the interaction.
    if (typeof window === 'undefined') return Promise.resolve(spec.kind === 'alert' ? null : false)
    const text = typeof spec.message === 'string' ? spec.message : (typeof spec.title === 'string' ? spec.title : '')
    if (spec.kind === 'alert') { window.alert(text); return Promise.resolve(null) }
    if (spec.kind === 'prompt') return Promise.resolve(window.prompt(text, spec.defaultValue ?? '') ?? null)
    return Promise.resolve(window.confirm(text))
  }
  return new Promise((resolve) => dispatch!(spec, resolve))
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

/** Mount once, at the app root, so the imperative dialog functions have somewhere to render. */
export function ConfirmHost() {
  const [spec, setSpec] = useState<DialogSpec | null>(null)
  const [value, setValue] = useState('')
  const resolveRef = useRef<Resolve | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    dispatch = (next, resolve) => {
      resolveRef.current = resolve
      setValue(next.defaultValue ?? '')
      setSpec(next)
    }
    return () => { dispatch = null }
  }, [])

  useEffect(() => {
    if (spec?.kind === 'prompt') {
      const id = requestAnimationFrame(() => inputRef.current?.select())
      return () => cancelAnimationFrame(id)
    }
  }, [spec])

  const settle = (result: boolean | string | null) => {
    resolveRef.current?.(result)
    resolveRef.current = null
    setSpec(null)
  }

  const cancel = () => settle(spec?.kind === 'prompt' ? null : false)
  const accept = () => settle(spec?.kind === 'confirm' ? true : spec?.kind === 'prompt' ? value : null)

  const isAlert = spec?.kind === 'alert'

  return (
    <Modal
      open={!!spec}
      onClose={cancel}
      title={spec?.title}
      footer={
        <div className="flex justify-end gap-2">
          {!isAlert && (
            <Button variant="ghost" size="sm" onClick={cancel}>
              {spec?.cancelLabel ?? 'Cancel'}
            </Button>
          )}
          <Button
            variant={spec?.danger ? 'danger-outline' : 'primary'}
            size="sm"
            onClick={accept}
          >
            {spec?.confirmLabel ?? (isAlert ? 'OK' : 'Confirm')}
          </Button>
        </div>
      }
    >
      {spec?.message && <div className="whitespace-pre-line text-sm text-muted">{spec.message}</div>}
      {spec?.kind === 'prompt' && (
        <Input
          ref={inputRef}
          value={value}
          placeholder={spec.placeholder}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); accept() } }}
          className={spec.message ? 'mt-4' : undefined}
          autoFocus
        />
      )}
    </Modal>
  )
}
