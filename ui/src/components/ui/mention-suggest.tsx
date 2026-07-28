import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from 'react'
import { Extension, type Editor, type Range } from '@tiptap/core'
import Suggestion, { type SuggestionOptions } from '@tiptap/suggestion'
import { ReactRenderer } from '@tiptap/react'
import { cn } from '@/lib/cn'
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
      <div ref={listRef} className="max-h-56 w-60 overflow-y-auto rounded-xl border border-line bg-card p-1 shadow-lg">
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
            className={cn('flex w-full items-baseline gap-2 rounded-lg px-2 py-1.5 text-left', i === active ? 'bg-sidebar' : 'hover:bg-sidebar')}
          >
            <span className="truncate text-xs font-medium text-fg">{item.label}</span>
            {item.sub && <span className="ml-auto truncate text-[11px] text-muted">{item.sub}</span>}
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
      return {
        onStart: (props) => {
          if (!props.items.length) return
          component = new ReactRenderer(MentionList, {
            props: { items: props.items, command: (item: Mentionable) => props.command(item) },
            editor: props.editor,
          })
          if (!props.clientRect) return
          popup = document.createElement('div')
          popup.style.position = 'fixed'
          popup.style.zIndex = '60'
          document.body.appendChild(popup)
          popup.appendChild(component.element)
          const rect = props.clientRect()
          if (rect) place(popup, rect)
        },
        onUpdate: (props) => {
          component?.updateProps({ items: props.items, command: (item: Mentionable) => props.command(item) })
          const rect = props.clientRect?.()
          if (popup && rect) place(popup, rect)
        },
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
    return [Suggestion({ editor: this.editor, ...buildSuggestion(() => this.options.items) })]
  },
})
