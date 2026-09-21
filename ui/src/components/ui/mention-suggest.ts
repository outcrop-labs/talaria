import { placeMenu } from '@/lib/menu-position'
import { flushSync, mount, unmount } from 'svelte'
import { Extension, type Editor, type Range } from '@tiptap/core'
import Suggestion, { type SuggestionOptions } from '@tiptap/suggestion'
import { PluginKey } from '@tiptap/pm/state'
import { menuHandlesKey } from '@/components/chat/composer-keys'
import MentionList from './MentionList.svelte'
import type { Mentionable } from '@/components/chat/mentions.svelte'

// "@" people-mention autocomplete for RichEditor — the TipTap counterpart of
// the old textarea composers' mention menu. Picks insert PLAIN TEXT "@token "
// (the exact grammar the server notifies on and the Markdown renderer
// highlights) — no special node, so the markdown round-trip is untouched.

function filterMentions(items: Mentionable[], query: string): Mentionable[] {
  const q = query.trim().toLowerCase()
  const pool = q ? items.filter((m) => m.label.toLowerCase().includes(q) || m.insert.toLowerCase().startsWith(q)) : items
  return pool.slice(0, 8)
}

/** The imperative surface MentionList.svelte exports (mount() returns it). */
interface MenuHandle {
  update: (items: Mentionable[], command: (item: Mentionable) => void) => void
  onKeyDown: (e: KeyboardEvent) => boolean
}



function buildSuggestion(items: () => Mentionable[]): Omit<SuggestionOptions<Mentionable>, 'editor'> {
  return {
    char: '@',
    allowSpaces: false,
    startOfLine: false,
    items: ({ query }) => filterMentions(items(), query),
    command: ({ editor, range, props }: { editor: Editor; range: Range; props: Mentionable }) => {
      editor.chain().focus().deleteRange(range).insertContent(`@${props.insert} `).run()
    },
    render: () => {
      let menu: MenuHandle | null = null
      let popup: HTMLDivElement | null = null
      // The latest candidate list the menu was rendered with — what onKeyDown
      // consults, so an armed-but-empty menu falls through instead of eating
      // Enter (TALA-5). Named apart from the config's `items` getter above.
      let current: Mentionable[] = []
      // Creatable from onUpdate too: if the first result set is empty (query
      // typed fast, filtered pool), onStart alone would never show the menu.
      const ensure = (props: { items: Mentionable[]; command: (item: Mentionable) => void; clientRect?: (() => DOMRect | null) | null }) => {
        current = props.items
        if (menu) {
          menu.update(props.items, (item: Mentionable) => props.command(item))
        } else if (props.items.length) {
          popup = document.createElement('div')
          popup.style.position = 'fixed'
          popup.style.zIndex = '60'
          document.body.appendChild(popup)
          menu = mount(MentionList, { target: popup }) as unknown as MenuHandle
          menu.update(props.items, (item: Mentionable) => props.command(item))
        }
        // Render synchronously so placeMenu() can measure the popup's height.
        flushSync()
        const rect = props.clientRect?.()
        if (popup && rect) placeMenu(popup, rect)
      }
      return {
        onStart: ensure,
        onUpdate: ensure,
        onKeyDown: (props) => {
          if (props.event.key === 'Escape') {
            popup?.remove()
            popup = null
            return true
          }
          // The pure decision (TALA-5): an empty menu claims NOTHING — Enter
          // falls through to the send keymap; a populated menu claims its
          // selection keys and the mounted component executes the pick.
          if (!menuHandlesKey(props.event.key, current)) return false
          return menu?.onKeyDown(props.event) ?? false
        },
        onExit: () => {
          popup?.remove()
          popup = null
          if (menu) {
            void unmount(menu as unknown as Record<string, unknown>)
            menu = null
          }
        },
      }
    },
  }
}

export interface MentionSuggestOptions {
  /** Candidates, read LIVE on every keystroke. A plain array here froze the
   *  mount-time list into the plugin: every surface builds its mentionables
   *  with `$derived` over async queries, so the picker showed whoever had
   *  resolved by editor creation — cached users, never the agents that land
   *  with the fleet — and an empty-at-mount list meant no picker at all. */
  items: () => Mentionable[]
}

/** Configure with `{ items: () => list }` at editor creation. */
export const MentionSuggest = Extension.create<MentionSuggestOptions>({
  name: 'mentionSuggest',
  addOptions() {
    return { items: () => [] }
  },
  addProseMirrorPlugins() {
    return [Suggestion({ editor: this.editor, pluginKey: new PluginKey('mentionSuggest'), ...buildSuggestion(this.options.items) })]
  },
})
