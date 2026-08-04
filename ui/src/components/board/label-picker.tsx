import { Combobox } from '@/components/ui/combobox'
import type { ControlSize } from '@/components/ui/control'

// Ticket labels as tag chips + the shared combobox: existing board tags surface
// in the dropdown for reuse; typing creates a new one (Enter or comma commits).
export function LabelPicker({
  value,
  options,
  onChange,
  disabled,
  size,
}: {
  value: string[]
  /** Tags already in use (e.g. across the board) — offered for reuse. */
  options: string[]
  onChange: (next: string[]) => void
  disabled?: boolean
  size?: ControlSize
}) {
  const opts = [...new Set([...options, ...value])]
    .sort()
    .map((t) => ({ value: t, label: t }))

  return (
    <div className="space-y-1.5">
      {value.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {value.map((t) => (
            <span key={t} className="flex items-center gap-1 rounded-full border border-line px-2 py-0.5 font-mono text-[10px] tracking-[0.05em] text-muted">
              {t}
              {!disabled && (
                <button
                  type="button"
                  aria-label={`Remove ${t}`}
                  onClick={() => onChange(value.filter((x) => x !== t))}
                  className="transition-colors hover:text-danger"
                >
                  ✕
                </button>
              )}
            </span>
          ))}
        </div>
      )}
      <Combobox
        options={opts}
        selected={value}
        onChange={onChange}
        multiple
        allowCreate
        disabled={disabled}
        size={size}
        triggerLabel={<span className="text-muted">Add label</span>}
      />
    </div>
  )
}
