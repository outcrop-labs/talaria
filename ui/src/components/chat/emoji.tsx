import { useEffect, useMemo, useRef, useState } from 'react'
import { Smile } from 'lucide-react'
import { cn } from '@/lib/cn'
import { Input } from '@/components/ui/input'
import { popPanel, popRow, tileBase } from '@/components/chat/chat-chrome'
import { EMOJI, searchEmoji, type EmojiEntry } from '@/lib/emoji'

// Emoji entry points for composers: the :shortcode: autocomplete (mirrors the
// @mention machinery) and the smiley button with a searchable grid.

export interface EmojiShortcodeState {
  /** Where the ":" starts in the input (for replacement on pick). */
  start: number
  options: EmojiEntry[]
}

/** ":word" (2+ chars) immediately before the caret opens the menu; the host
 *  feeds keydowns here after the mention hook and renders <EmojiShortcodeMenu>. */
export function useEmojiShortcodes(
  input: string,
  caret: number,
  apply: (next: string, caretPos: number) => void,
) {
  const [picked, setPicked] = useState(0)
  const [dismissed, setDismissed] = useState<number | null>(null)

  const state = useMemo<EmojiShortcodeState | null>(() => {
    const upto = input.slice(0, caret)
    const m = /(^|\s):([a-z0-9_+-]{2,})$/i.exec(upto)
    if (!m) return null
    const start = upto.length - m[2]!.length - 1
    if (dismissed === start) return null
    const options = searchEmoji(m[2]!, 8)
    return options.length ? { start, options } : null
  }, [input, caret, dismissed])

  useEffect(() => setPicked(0), [state?.options.length])
  // A new token position clears an old dismissal.
  useEffect(() => {
    if (dismissed !== null && state === null) return
  }, [dismissed, state])

  const insert = (ch: string) => {
    if (!state) return
    const next = `${input.slice(0, state.start)}${ch} ${input.slice(caret)}`
    apply(next, state.start + ch.length + 1)
  }

  /** Returns true when the menu consumed the key. */
  const onKeyDown = (e: React.KeyboardEvent): boolean => {
    if (!state) return false
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      const d = e.key === 'ArrowDown' ? 1 : -1
      setPicked((p) => (p + d + state.options.length) % state.options.length)
      return true
    }
    if (e.key === 'Tab' || e.key === 'Enter') {
      e.preventDefault()
      insert(state.options[picked]!.ch)
      return true
    }
    if (e.key === 'Escape') {
      setDismissed(state.start)
      return true
    }
    return false
  }

  return { emoji: state, emojiPicked: picked, insertEmoji: insert, onEmojiKeyDown: onKeyDown }
}

export function EmojiShortcodeMenu({
  state,
  picked,
  onPick,
  className,
}: {
  state: EmojiShortcodeState
  picked: number
  onPick: (ch: string) => void
  className?: string
}) {
  return (
    <div className={cn(popPanel, 'z-10 w-56 overflow-hidden', className)}>
      {state.options.map((e, i) => (
        <button
          key={e.ch}
          type="button"
          onMouseDown={(ev) => {
            ev.preventDefault()
            onPick(e.ch)
          }}
          className={cn(popRow, i === picked ? 'bg-hover text-fg' : 'text-muted')}
        >
          <span className="text-base">{e.ch}</span>
          <span className="truncate font-mono text-[11px] tracking-[0.02em]">:{e.names[0]}:</span>
        </button>
      ))}
    </div>
  )
}

/** The composer's smiley button: a searchable grid, click to insert. */
export function EmojiButton({ onPick, disabled }: { onPick: (ch: string) => void; disabled?: boolean }) {
  const [open, setOpen] = useState(false)
  const [q, setQ] = useState('')
  const wrapRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDoc = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDoc)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDoc)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const results = q.trim() ? searchEmoji(q, 40) : EMOJI.slice(0, 40)

  return (
    <div ref={wrapRef} className="relative">
      <button
        type="button"
        title="Add emoji (or type :shortcode:)"
        disabled={disabled}
        onClick={() => {
          setOpen((v) => !v)
          setQ('')
        }}
        className={tileBase}
      >
        <Smile size={15} />
      </button>
      {open && (
        <div className={cn(popPanel, 'absolute bottom-full left-0 z-30 mb-1.5 w-72 p-2')}>
          <Input
            autoFocus
            size="sm"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="Search emoji"
            className="mb-2"
          />
          <div className="grid max-h-48 grid-cols-8 gap-0.5 overflow-y-auto">
            {results.map((e) => (
              <button
                key={e.ch}
                type="button"
                title={`:${e.names[0]}:`}
                onClick={() => {
                  onPick(e.ch)
                  setOpen(false)
                }}
                className="grid h-8 w-8 place-items-center rounded-md text-lg transition-colors hover:bg-hover"
              >
                {e.ch}
              </button>
            ))}
            {results.length === 0 && <div className="col-span-8 px-1 py-2 text-xs text-muted">No matches</div>}
          </div>
        </div>
      )}
    </div>
  )
}
