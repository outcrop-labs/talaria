import { forwardRef, useEffect, useImperativeHandle, useState } from 'react'
import { Extension, type Editor, type Range } from '@tiptap/core'
import Suggestion, { type SuggestionOptions } from '@tiptap/suggestion'
import { PluginKey } from '@tiptap/pm/state'
import { ReactRenderer } from '@tiptap/react'
import { cn } from '@/lib/cn'
import { popPanel, popRow } from '@/components/chat/chat-chrome'
import { searchEmoji, type EmojiEntry } from '@/lib/emoji'

// ":" emoji autocomplete for the chat composer — the TipTap counterpart of
// the textarea composers' useEmojiShortcodes. Picks insert the emoji CHARACTER
// (not a node), so the markdown round-trip is untouched. Two typed characters
// arm it (":ro" → 🚀), which keeps ordinary colons in prose quiet.

interface MenuHandle {
  onKeyDown: (e: KeyboardEvent) => boolean
}

const EmojiList = forwardRef<MenuHandle, { items: EmojiEntry[]; command: (item: EmojiEntry) => void }>(
  function EmojiList({ items, command }, ref) {
    const [active, setActive] = useState(0)
    useEffect(() => setActive(0), [items])

    useImperativeHandle(ref, () => ({
      onKeyDown: (e) => {
        if (e.key === 'ArrowDown') {
          setActive((a) => (a + 1) % Math.max(items.length, 1))
          return true
        }
        if (e.key === 'ArrowUp') {
          setActive((a) => (a - 1 + items.length) % Math.max(items.length, 1))
          return true
        }
        if (e.key === 'Enter' || e.key === 'Tab') {
          if (items[active]) command(items[active])
          return true
        }
        return false
      },
    }))

    if (items.length === 0) return null
    return (
      <div className={cn(popPanel, 'w-56')}>
        {items.map((item, i) => (
          <button
            key={item.ch}
            type="button"
            onMouseEnter={() => setActive(i)}
            onMouseDown={(e) => {
              e.preventDefault()
              command(item)
            }}
            className={cn(popRow, i === active && 'bg-hover')}
          >
            <span className="text-base">{item.ch}</span>
            <span className="truncate font-mono text-[11px] tracking-[0.02em] text-muted">:{item.names[0]}:</span>
          </button>
        ))}
      </div>
    )
  },
)

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
    let component: ReactRenderer<MenuHandle> | null = null
    let popup: HTMLDivElement | null = null
    // The ":" trigger fires with an EMPTY query (zero items — we require two
    // typed characters), so the menu must be creatable from onUpdate too, once
    // matches exist. onStart alone would race and never show anything.
    const ensure = (props: { items: EmojiEntry[]; command: (item: EmojiEntry) => void; editor: Editor; clientRect?: (() => DOMRect | null) | null }) => {
      if (component) {
        component.updateProps({ items: props.items, command: (item: EmojiEntry) => props.command(item) })
      } else if (props.items.length) {
        component = new ReactRenderer(EmojiList, {
          props: { items: props.items, command: (item: EmojiEntry) => props.command(item) },
          editor: props.editor,
        })
        popup = document.createElement('div')
        popup.style.position = 'fixed'
        popup.style.zIndex = '60'
        document.body.appendChild(popup)
        popup.appendChild(component.element)
      }
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
        return component?.ref?.onKeyDown(props.event) ?? false
      },
      onExit: () => {
        popup?.remove()
        popup = null
        component?.destroy()
        component = null
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
