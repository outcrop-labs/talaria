import { useRef, useState } from 'react'
import { motion } from 'framer-motion'
import { Plus, CornerDownLeft } from 'lucide-react'
import { Button } from './button'
import { Input } from './input'

/** A primary "+ <label>" button that expands into a full-width input. Enter (or
 *  blur) submits, Escape cancels. Reuse anywhere you want an unobtrusive create
 *  affordance that only takes space while in use. */
export function InlineCreate({
  label,
  placeholder,
  onSubmit,
  className,
  size = 'sm',
}: {
  label: string
  placeholder?: string
  onSubmit: (value: string) => void
  className?: string
  size?: 'sm' | 'md'
}) {
  const [open, setOpen] = useState(false)
  const [value, setValue] = useState('')
  const cancel = useRef(false)

  const submit = () => {
    setOpen(false)
    const v = value.trim()
    const cancelled = cancel.current
    cancel.current = false
    setValue('')
    if (v && !cancelled) onSubmit(v)
  }

  if (!open) {
    return (
      <div className={className}>
        <Button size={size} onClick={() => setOpen(true)}>
          <Plus size={size === 'sm' ? 15 : 17} />
          {label}
        </Button>
      </div>
    )
  }

  return (
    <motion.div
      className={className}
      initial={{ opacity: 0, scaleX: 0.55 }}
      animate={{ opacity: 1, scaleX: 1 }}
      transition={{ duration: 0.15 }}
      style={{ transformOrigin: 'left' }}
    >
      <div className="relative">
        <Input
          autoFocus
          value={value}
          placeholder={placeholder ?? label}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') e.currentTarget.blur()
            else if (e.key === 'Escape') {
              cancel.current = true
              e.currentTarget.blur()
            }
          }}
          onBlur={submit}
          size={size}
          className={size === 'sm' ? 'pr-8' : 'pr-9'}
        />
        {/* Enter hint — pressing Enter (or clicking) submits. mousedown fires
            before the input's blur so the value is still there. */}
        <button
          type="button"
          tabIndex={-1}
          aria-label="Submit"
          onMouseDown={(e) => {
            e.preventDefault()
            ;(e.currentTarget.previousElementSibling as HTMLInputElement | null)?.blur()
          }}
          className="absolute inset-y-0 right-2 grid place-items-center text-muted transition-colors hover:text-accent"
        >
          <CornerDownLeft size={size === 'sm' ? 14 : 16} />
        </button>
      </div>
    </motion.div>
  )
}
