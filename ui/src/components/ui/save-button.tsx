// The one "Save → Saved ✓" pattern. useSavedFlash gives the timed flag;
// SaveButton bundles it with the standard button + confirmation copy so
// settings panels stop re-rolling setTimeout state machines.
import { useEffect, useRef, useState } from 'react'
import { Button, type ButtonProps } from './button'
import { cn } from '@/lib/cn'

export function useSavedFlash(ms = 1500): { saved: boolean; flash: () => void } {
  const [saved, setSaved] = useState(false)
  const t = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(
    () => () => {
      if (t.current) clearTimeout(t.current)
    },
    [],
  )
  return {
    saved,
    flash: () => {
      setSaved(true)
      if (t.current) clearTimeout(t.current)
      t.current = setTimeout(() => setSaved(false), ms)
    },
  }
}

/** Runs `onSave`, disables while pending, flashes "Saved". */
export function SaveButton({
  onSave,
  children = 'Save',
  className,
  disabled,
  ...rest
}: Omit<ButtonProps, 'onClick'> & { onSave: () => Promise<unknown> | unknown }) {
  const [busy, setBusy] = useState(false)
  const { saved, flash } = useSavedFlash()
  return (
    <span className={cn('inline-flex items-center gap-2', className)}>
      <Button
        {...rest}
        disabled={disabled || busy}
        onClick={() => {
          setBusy(true)
          void Promise.resolve(onSave())
            .then(() => flash())
            .finally(() => setBusy(false))
        }}
      >
        {children}
      </Button>
      {saved && <span className="text-xs text-[color:var(--theme-success)]">Saved</span>}
    </span>
  )
}
