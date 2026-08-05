import { flushSync, mount, unmount } from 'svelte'
import { Extension, type Editor, type Range } from '@tiptap/core'
import Suggestion, { type SuggestionOptions } from '@tiptap/suggestion'
import { PluginKey } from '@tiptap/pm/state'
import EmojiList from './EmojiList.svelte'
import { searchEmoji, type EmojiEntry } from '@/lib/emoji'

// ":" emoji autocomplete for the chat composer — the TipTap counterpart of
// the textarea composers' useEmojiShortcodes. Picks insert the emoji CHARACTER
// (not a node), so the markdown round-trip is untouched. Two typed characters
// arm it (":ro" → 🚀), which keeps ordinary colons in prose quiet.

/** The imperative surface EmojiList.svelte exports (mount() returns it). */
interface MenuHandle {
  update: (items: EmojiEntry[], command: (item: EmojiEntry) => void) => void
  onKeyDown: (e: KeyboardEvent) => boolean
}

function place(el: HTMLElement, rect: DOMRect) {
  const margin = 6
  el.style.left = `${rect.left}px`
  const below = rect.bottom + margin
  if (below + el.offsetHeight > window.innerHeight && rect.top - margin - el.offsetHeight > 0) {
    el.style.top = `${rect.top - margin - el.offsetHeight}px`
  } else {
    el.style.top = `${below}px`
  }
}

const suggestion: Omit<SuggestionOptions<EmojiEntry>, 'editor'> = {
  char: ':',
  allowSpaces: false,
  startOfLine: false,
  items: ({ query }) => (query.length >= 2 ? searchEmoji(query, 8) : []),
  command: ({ editor, range, props }: { editor: Editor; range: Range; props: EmojiEntry }) => {
    editor.chain().focus().deleteRange(range).insertContent(`${props.ch} `).run()
  },
  render: () => {
    let menu: MenuHandle | null = null
    let popup: HTMLDivElement | null = null
    // The ":" trigger fires with an EMPTY query (zero items — we require two
    // typed characters), so the menu must be creatable from onUpdate too, once
    // matches exist. onStart alone would race and never show anything.
    const ensure = (props: { items: EmojiEntry[]; command: (item: EmojiEntry) => void; clientRect?: (() => DOMRect | null) | null }) => {
      if (menu) {
        menu.update(props.items, (item: EmojiEntry) => props.command(item))
      } else if (props.items.length) {
        popup = document.createElement('div')
        popup.style.position = 'fixed'
        popup.style.zIndex = '60'
        document.body.appendChild(popup)
        menu = mount(EmojiList, { target: popup }) as unknown as MenuHandle
        menu.update(props.items, (item: EmojiEntry) => props.command(item))
      }
      // Render synchronously so place() can measure the popup's height.
      flushSync()
      const rect = props.clientRect?.()
      if (popup && rect) place(popup, rect)
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

export const EmojiSuggest = Extension.create({
  name: 'emojiSuggest',
  addProseMirrorPlugins() {
    return [Suggestion({ editor: this.editor, pluginKey: new PluginKey('emojiSuggest'), ...suggestion })]
  },
})
