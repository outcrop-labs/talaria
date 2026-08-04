import { useState } from 'react'
import { Link2, Copy, Check } from 'lucide-react'
import { cn } from '@/lib/cn'
import { focusRing } from './control'

/** The one copy-to-clipboard affordance: icon (or icon+label) that flashes a
 *  check. Pass `value` for literal text, or `path` for an app link (origin
 *  resolved at click time, SSR-safe). Stops propagation so it never triggers
 *  the surrounding card/row click. */
export function CopyButton({
  value,
  path,
  label,
  className,
  title = 'Copy',
}: {
  /** Literal text to copy. */
  value?: string
  /** App path — copied as a full URL. */
  path?: string
  label?: string
  className?: string
  title?: string
}) {
  const [copied, setCopied] = useState(false)
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={(e) => {
        e.preventDefault()
        e.stopPropagation()
        const text = value ?? (path ? window.location.origin + path : '')
        if (!text) return
        void navigator.clipboard?.writeText(text)
        setCopied(true)
        setTimeout(() => setCopied(false), 1200)
      }}
      className={cn(
        // Ghost icon affordance (spec §8): muted → readout, hover-token fill.
        'flex items-center gap-1 rounded-md p-1 text-muted transition-colors hover:bg-hover hover:text-fg',
        focusRing,
        className,
      )}
    >
      {copied ? <Check size={13} /> : path ? <Link2 size={13} /> : <Copy size={13} />}
      {label && <span>{copied ? 'Copied' : label}</span>}
    </button>
  )
}

/** Copies an absolute link to `path` — thin wrapper kept for existing call
 *  sites; new code can use CopyButton directly. */
export function CopyLinkButton({
  path,
  label,
  className,
  title = 'Copy link',
}: {
  path: string
  label?: string
  className?: string
  title?: string
}) {
  return <CopyButton path={path} label={label} className={className} title={title} />
}
