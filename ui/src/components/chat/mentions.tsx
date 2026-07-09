import { useEffect, useMemo, useState } from 'react'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'

/** A composer mention option: `insert` is the token typed into the message. */
export interface Mentionable {
  insert: string
  label: string
  sub?: string
}

export interface MentionState {
  /** Where the "@" starts in the input (for replacement on pick). */
  start: number
  options: Mentionable[]
}

/** Shared @mention machinery for composers (channels, plan, …). An "@word"
 *  immediately before the caret opens the menu; the host feeds keydowns here
 *  FIRST (returns true when the menu consumed the key) and renders
 *  <MentionMenu> above its input. */
export function useMentions(
  input: string,
  caret: number,
  setCaret: (n: number) => void,
  mentionables: Mentionable[],
  apply: (next: string, caretPos: number) => void,
) {
  const [picked, setPicked] = useState(0)

  const mention = useMemo<MentionState | null>(() => {
    if (mentionables.length === 0) return null
    const upto = input.slice(0, caret)
    const m = /(^|\s)@([a-z0-9-]*(?::[a-z0-9-]*)?)$/i.exec(upto)
    if (!m) return null
    const q = m[2]!.toLowerCase()
    const options = mentionables.filter(
      (a) => a.label.toLowerCase().startsWith(q) || a.insert.toLowerCase().startsWith(q),
    )
    return options.length ? { start: upto.length - m[2]!.length - 1, options } : null
  }, [input, caret, mentionables])

  useEffect(() => setPicked(0), [mention?.options.length])

  const insert = (token: string) => {
    if (!mention) return
    const next = `${input.slice(0, mention.start)}@${token} ${input.slice(caret)}`
    apply(next, mention.start + token.length + 2)
  }

  /** Returns true when the menu consumed the key (host must not also act on it). */
  const onKeyDown = (e: React.KeyboardEvent): boolean => {
    if (!mention) return false
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      const d = e.key === 'ArrowDown' ? 1 : -1
      setPicked((p) => (p + d + mention.options.length) % mention.options.length)
      return true
    }
    if (e.key === 'Tab' || e.key === 'Enter') {
      e.preventDefault()
      insert(mention.options[picked]!.insert)
      return true
    }
    if (e.key === 'Escape') {
      setCaret(0)
      return true
    }
    return false
  }

  return { mention, picked, insert, onKeyDown }
}

/** The mention dropdown panel. Position it with `className` (host-specific). */
export function MentionMenu({
  mention,
  picked,
  onPick,
  className,
}: {
  mention: MentionState
  picked: number
  onPick: (insert: string) => void
  className?: string
}) {
  return (
    <div className={cn('mercury-panel z-10 w-64 overflow-hidden rounded-xl p-1', className)}>
      {mention.options.map((a, i) => (
        <button
          key={`${a.insert}-${a.sub ?? ''}`}
          type="button"
          onMouseDown={(e) => {
            e.preventDefault()
            onPick(a.insert)
          }}
          className={cn(
            'flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm',
            i === picked ? 'bg-card text-fg' : 'text-muted',
          )}
        >
          <Avatar name={a.label} className="h-5 w-5 text-xs" />
          <span className="truncate">{a.label}</span>
          {a.sub && <span className="ml-auto truncate text-xs text-muted">{a.sub}</span>}
        </button>
      ))}
    </div>
  )
}

/** The composer token for @mentioning a user — mirrors the server's
 *  mention tokens (email localpart, else dashed full name). */
export const userMentionInsert = (u: { name: string | null; email: string | null }): string =>
  u.email?.split('@')[0] ?? (u.name ?? '').toLowerCase().replace(/\s+/g, '-')
