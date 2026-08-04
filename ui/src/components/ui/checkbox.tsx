// Checkbox / Radio / Toggle — the accent-styled selection controls. Native
// inputs underneath (keyboard + a11y for free), warm-gold accent on top; the
// label is part of the control so the hit target is the whole row.
import { cn } from '@/lib/cn'

// Spec §8 focus-visible: solid gold ring on controls.
const focusRing = 'focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2'

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
        className={cn('accent-accent', focusRing)}
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
        className={cn('accent-accent', focusRing)}
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
          // Spec §8 toggle: pill ~36×20; on = gold knob on a warm track;
          // off = muted knob on a raised track; disabled rows dim (label).
          'relative inline-block h-5 w-9 shrink-0 rounded-full border transition-colors',
          focusRing,
          checked ? 'border-[var(--theme-accent-border)] bg-accent-soft' : 'border-line bg-raised',
        )}
      >
        <span
          className={cn(
            'absolute top-1/2 h-3.5 w-3.5 -translate-y-1/2 rounded-full transition-all',
            checked ? 'left-[18px] bg-accent' : 'left-0.5 bg-muted',
          )}
        />
      </span>
      {label}
    </label>
  )
}
