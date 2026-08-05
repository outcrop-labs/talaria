import { searchEmoji, type EmojiEntry } from '@/lib/emoji'

// Emoji entry points for composers: the :shortcode: autocomplete (mirrors the
// @mention machinery — see mentions.svelte.ts) and the smiley button with a
// searchable grid (EmojiButton.svelte). The menu itself renders from
// EmojiShortcodeMenu.svelte.

export interface EmojiShortcodeState {
  /** Where the ":" starts in the input (for replacement on pick). */
  start: number
  options: EmojiEntry[]
}

/** ":word" (2+ chars) immediately before the caret opens the menu; the host
 *  feeds keydowns here after the mention hook and renders <EmojiShortcodeMenu>.
 *
 *  Call during component init. `input`/`caret` are getters so the runes they
 *  read stay live; read `emoji`/`emojiPicked` off the returned object
 *  (destructuring would freeze them). */
export function useEmojiShortcodes(
  input: () => string,
  caret: () => number,
  apply: (next: string, caretPos: number) => void,
) {
  let picked = $state(0)
  let dismissed = $state<number | null>(null)

  const state = $derived.by<EmojiShortcodeState | null>(() => {
    const upto = input().slice(0, caret())
    const m = /(^|\s):([a-z0-9_+-]{2,})$/i.exec(upto)
    if (!m) return null
    const start = upto.length - m[2]!.length - 1
    if (dismissed === start) return null
    const options = searchEmoji(m[2]!, 8)
    return options.length ? { start, options } : null
  })

  // New result COUNT → selection back to the top. (Keyed on the count, as the
  // React effect was, so a same-size filter change keeps the selection.)
  let lastCount: number | undefined
  $effect(() => {
    const count = state?.options.length
    if (count !== lastCount) {
      lastCount = count
      picked = 0
    }
  })

  // A new token position clears an old dismissal — `dismissed` only ever
  // matches its own token's `start`, so a token at a fresh offset naturally
  // escapes the suppression. (The React version carried a no-op effect here.)

  const insert = (ch: string) => {
    const s = state
    if (!s) return
    const text = input()
    const next = `${text.slice(0, s.start)}${ch} ${text.slice(caret())}`
    apply(next, s.start + ch.length + 1)
  }

  /** Returns true when the menu consumed the key. */
  const onKeyDown = (e: KeyboardEvent): boolean => {
    const s = state
    if (!s) return false
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      const d = e.key === 'ArrowDown' ? 1 : -1
      picked = (picked + d + s.options.length) % s.options.length
      return true
    }
    if (e.key === 'Tab' || e.key === 'Enter') {
      e.preventDefault()
      insert(s.options[picked]!.ch)
      return true
    }
    if (e.key === 'Escape') {
      dismissed = s.start
      return true
    }
    return false
  }

  return {
    get emoji() {
      return state
    },
    get emojiPicked() {
      return picked
    },
    insertEmoji: insert,
    onEmojiKeyDown: onKeyDown,
  }
}
