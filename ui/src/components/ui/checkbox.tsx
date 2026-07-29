// Checkbox / Radio / Toggle — the accent-styled selection controls. Native
// inputs underneath (keyboard + a11y for free), Mercury accent on top; the
// label is part of the control so the hit target is the whole row.
import { cn } from '@/lib/cn'

interface BaseProps {
  checked: boolean
  onChange: (checked: boolean) => void
  label?: React.ReactNode
  disabled?: boolean
  title?: string
  className?: string
}

export function Checkbox({ checked, onChange, label, disabled, title, className }: BaseProps) {
  return (
    <label title={title} className={cn('flex cursor-pointer items-center gap-1.5 text-xs text-muted', disabled && 'cursor-default opacity-50', className)}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="accent-[var(--theme-accent)]"
      />
      {label}
    </label>
  )
}

export function Radio({ checked, onChange, label, disabled, title, className, name }: BaseProps & { name?: string }) {
  return (
    <label title={title} className={cn('flex cursor-pointer items-center gap-1.5 text-xs text-muted', disabled && 'cursor-default opacity-50', className)}>
      <input
        type="radio"
        name={name}
        checked={checked}
        disabled={disabled}
        onChange={(e) => e.target.checked && onChange(true)}
        className="accent-[var(--theme-accent)]"
      />
      {label}
    </label>
  )
}

/** Switch-styled boolean — same API as Checkbox, heavier visual weight for
 *  settings rows where the toggle IS the feature. */
export function Toggle({ checked, onChange, label, disabled, title, className }: BaseProps) {
  return (
    <label title={title} className={cn('flex cursor-pointer items-center gap-2 text-xs text-muted', disabled && 'cursor-default opacity-50', className)}>
      <span
        role="switch"
        aria-checked={checked}
        tabIndex={disabled ? -1 : 0}
        onKeyDown={(e) => {
          if (disabled) return
          if (e.key === ' ' || e.key === 'Enter') {
            e.preventDefault()
            onChange(!checked)
          }
        }}
        onClick={() => !disabled && onChange(!checked)}
        className={cn(
          'relative inline-block h-4 w-7 shrink-0 rounded-full border transition-colors',
          checked ? 'border-[var(--theme-accent-border)] bg-accent/80' : 'border-line bg-card',
        )}
      >
        <span
          className={cn(
            'absolute top-1/2 h-3 w-3 -translate-y-1/2 rounded-full transition-all',
            checked ? 'left-3.5 bg-surface' : 'left-0.5 bg-muted',
          )}
        />
      </span>
      {label}
    </label>
  )
}
