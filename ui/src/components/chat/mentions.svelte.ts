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

/** Shared @mention machinery for composers (channels, plan, ). An "@word"
 *  immediately before the caret opens the menu; the host feeds keydowns here
 *  FIRST (returns true when the menu consumed the key) and renders
 *  <MentionMenu> above its input.
 *
 *  Call during component init. `input`/`caret`/`mentionables` are getters so
 *  the runes they read stay live; read `mention`/`picked` off the returned
 *  object (destructuring would freeze them). */
export function useMentions(
  input: () => string,
  caret: () => number,
  setCaret: (n: number) => void,
  mentionables: () => Mentionable[],
  apply: (next: string, caretPos: number) => void,
) {
  let picked = $state(0)

  const mention = $derived.by<MentionState | null>(() => {
    const pool = mentionables()
    if (pool.length === 0) return null
    const upto = input().slice(0, caret())
    const m = /(^|\s)@([a-z0-9-]*(?::[a-z0-9-]*)?)$/i.exec(upto)
    if (!m) return null
    const q = m[2]!.toLowerCase()
    const options = pool.filter(
      (a) => a.label.toLowerCase().startsWith(q) || a.insert.toLowerCase().startsWith(q),
    )
    return options.length ? { start: upto.length - m[2]!.length - 1, options } : null
  })

  // New result COUNT → selection back to the top. (Keyed on the count, as the
  // React effect was, so a same-size filter change keeps the selection.)
  let lastCount: number | undefined
  $effect(() => {
    const count = mention?.options.length
    if (count !== lastCount) {
      lastCount = count
      picked = 0
    }
  })

  const insert = (token: string) => {
    const m = mention
    if (!m) return
    const text = input()
    const next = `${text.slice(0, m.start)}@${token} ${text.slice(caret())}`
    apply(next, m.start + token.length + 2)
  }

  /** Returns true when the menu consumed the key (host must not also act on it). */
  const onKeyDown = (e: KeyboardEvent): boolean => {
    const m = mention
    if (!m) return false
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      const d = e.key === 'ArrowDown' ? 1 : -1
      picked = (picked + d + m.options.length) % m.options.length
      return true
    }
    if (e.key === 'Tab' || e.key === 'Enter') {
      e.preventDefault()
      insert(m.options[picked]!.insert)
      return true
    }
    if (e.key === 'Escape') {
      setCaret(0)
      return true
    }
    return false
  }

  return {
    get mention() {
      return mention
    },
    get picked() {
      return picked
    },
    insert,
    onKeyDown,
  }
}

/** The composer token for @mentioning a user — mirrors the server's
 *  mention tokens (email localpart, else dashed full name). */
export const userMentionInsert = (u: { name: string | null; email: string | null }): string =>
  u.email?.split('@')[0] ?? (u.name ?? '').toLowerCase().replace(/\s+/g, '-')
