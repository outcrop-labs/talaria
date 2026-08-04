import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from 'react'
import { Extension, type Editor, type Range } from '@tiptap/core'
import Suggestion, { type SuggestionOptions } from '@tiptap/suggestion'
import { PluginKey } from '@tiptap/pm/state'
import { ReactRenderer } from '@tiptap/react'
import { cn } from '@/lib/cn'
import { popPanel } from '@/components/chat/chat-chrome'
import type { Mentionable } from '@/components/chat/mentions'

// "@" people-mention autocomplete for RichEditor — the TipTap counterpart of
// the textarea composers' useMentions/MentionMenu (chat/mentions.tsx). Picks
// insert PLAIN TEXT "@token " (the exact grammar the server notifies on and
// the Markdown renderer highlights) — no special node, so the markdown
// round-trip is untouched. Modeled on slash-commands.tsx.

function filterMentions(items: Mentionable[], query: string): Mentionable[] {
  const q = query.trim().toLowerCase()
  const pool = q ? items.filter((m) => m.label.toLowerCase().includes(q) || m.insert.toLowerCase().startsWith(q)) : items
  return pool.slice(0, 8)
}

interface MenuHandle {
  onKeyDown: (e: KeyboardEvent) => boolean
}

const MentionList = forwardRef<MenuHandle, { items: Mentionable[]; command: (item: Mentionable) => void }>(
  function MentionList({ items, command }, ref) {
    const [active, setActive] = useState(0)
    const listRef = useRef<HTMLDivElement>(null)
    useEffect(() => setActive(0), [items])
    useLayoutEffect(() => {
      listRef.current?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' })
    }, [active])

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
      <div ref={listRef} className={cn(popPanel, 'max-h-56 w-60 overflow-y-auto')}>
        {items.map((item, i) => (
          <button
            key={item.insert}
            type="button"
            data-active={i === active}
            onMouseEnter={() => setActive(i)}
            onMouseDown={(e) => {
              e.preventDefault()
              command(item)
            }}
            className={cn('flex w-full items-baseline gap-2 rounded-md px-2 py-1.5 text-left transition-colors', i === active ? 'bg-hover' : 'hover:bg-hover')}
          >
            <span className="truncate font-sans text-[13px] text-fg">{item.label}</span>
            {item.sub && <span className="ml-auto truncate font-mono text-[10px] tracking-[0.05em] text-muted">{item.sub}</span>}
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
  const wouldOverflow = below + el.offsetHeight > window.innerHeight
  if (wouldOverflow && rect.top - margin - el.offsetHeight > 0) {
    el.style.top = `${rect.top - margin - el.offsetHeight}px`
  } else {
    el.style.top = `${below}px`
  }
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
      let component: ReactRenderer<MenuHandle> | null = null
      let popup: HTMLDivElement | null = null
      // Creatable from onUpdate too: if the first result set is empty (query
      // typed fast, filtered pool), onStart alone would never show the menu.
      const ensure = (props: { items: Mentionable[]; command: (item: Mentionable) => void; editor: Editor; clientRect?: (() => DOMRect | null) | null }) => {
        if (component) {
          component.updateProps({ items: props.items, command: (item: Mentionable) => props.command(item) })
        } else if (props.items.length) {
          component = new ReactRenderer(MentionList, {
            props: { items: props.items, command: (item: Mentionable) => props.command(item) },
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
}

export interface MentionSuggestOptions {
  items: Mentionable[]
}

/** Configure with `{ items }` at editor creation (RichEditor remounts by key,
 *  so a fresh candidate list arrives with the remount). */
export const MentionSuggest = Extension.create<MentionSuggestOptions>({
  name: 'mentionSuggest',
  addOptions() {
    return { items: [] }
  },
  addProseMirrorPlugins() {
    return [Suggestion({ editor: this.editor, pluginKey: new PluginKey('mentionSuggest'), ...buildSuggestion(() => this.options.items) })]
  },
})
