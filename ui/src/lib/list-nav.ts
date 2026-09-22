// Arrow/Enter navigation for a suggestion menu's rows.
//
// EmojiList.svelte and MentionList.svelte each carried this verbatim: the same
// three keys, the same wrap-around, the same "Enter with nothing armed falls
// through" guard. They are two menus for two `Suggestion` chars ("@", ":")
// rendered imperatively by tiptap, so they cannot share a component — but they
// can share the key grammar, which is the part that has to agree.

/** The outcome of one key: the row that should now be active, and whether the
 *  key was consumed (a menu that eats Enter without choosing anything is the
 *  bug this return value exists to prevent — see the TALA-5 note in
 *  mention-suggest.ts). */
export interface ListNavResult {
  handled: boolean
  active: number
}

/** `choose` fires on Enter/Tab with the ACTIVE INDEX; the caller decides
 *  whether that index holds anything. */
export function listNav(
  e: KeyboardEvent,
  opts: { length: number; active: number; choose: (index: number) => void },
): ListNavResult {
  const last = Math.max(opts.length - 1, 0)
  if (e.key === 'ArrowDown') return { handled: true, active: opts.active >= last ? 0 : opts.active + 1 }
  if (e.key === 'ArrowUp') return { handled: true, active: opts.active <= 0 ? last : opts.active - 1 }
  if (e.key === 'Enter' || e.key === 'Tab') {
    if (opts.length) opts.choose(opts.active)
    return { handled: true, active: opts.active }
  }
  return { handled: false, active: opts.active }
}

/** The same grammar bound to a menu's live state — the shape both tiptap
 *  suggestion menus need, because `mount()` returns the handle and the menu
 *  lives outside any component tree. The component owns the state (runes) and
 *  hands over accessors; this owns the keys. */
export interface ListNav {
  onKeyDown(e: KeyboardEvent): boolean
  reset(): void
}

export function createListNav<T>(opts: {
  items: () => T[]
  active: () => number
  setActive: (index: number) => void
  choose: (item: T) => void
}): ListNav {
  return {
    onKeyDown(e) {
      const r = listNav(e, {
        length: opts.items().length,
        active: opts.active(),
        // A list that shrank under the cursor: choose only what is really there.
        choose: (i) => {
          const item = opts.items()[i]
          if (item) opts.choose(item)
        },
      })
      opts.setActive(r.active)
      return r.handled
    },
    reset() {
      opts.setActive(0)
    },
  }
}
