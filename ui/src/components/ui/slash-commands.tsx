import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from 'react'
import { Extension, type Editor, type Range } from '@tiptap/core'
import Suggestion, { type SuggestionOptions } from '@tiptap/suggestion'
import { ReactRenderer } from '@tiptap/react'
import {
  Heading1, Heading2, Heading3, List, ListOrdered, ListChecks, Quote,
  SquareCode, Table as TableIcon, Image as ImageIcon, Minus, Type,
  type LucideIcon,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { prompt } from './confirm'

// A slash-command menu (like Outline's block menu): type "/" to insert a block.
// Filterable, keyboard-navigable, positioned at the caret. Built on TipTap's
// Suggestion utility; the item set maps to the nodes our editor supports.

interface SlashItem {
  title: string
  hint: string
  icon: LucideIcon
  keywords: string[]
  run: (editor: Editor, range: Range) => void
}

const ITEMS: SlashItem[] = [
  { title: 'Text', hint: 'Plain paragraph', icon: Type, keywords: ['paragraph', 'body', 'p'], run: (e, r) => e.chain().focus().deleteRange(r).setParagraph().run() },
  { title: 'Big heading', hint: 'Large section heading', icon: Heading1, keywords: ['h1', 'title', 'heading1'], run: (e, r) => e.chain().focus().deleteRange(r).setHeading({ level: 1 }).run() },
  { title: 'Medium heading', hint: 'Section heading', icon: Heading2, keywords: ['h2', 'heading2', 'subtitle'], run: (e, r) => e.chain().focus().deleteRange(r).setHeading({ level: 2 }).run() },
  { title: 'Small heading', hint: 'Subsection heading', icon: Heading3, keywords: ['h3', 'heading3'], run: (e, r) => e.chain().focus().deleteRange(r).setHeading({ level: 3 }).run() },
  { title: 'Todo list', hint: 'Track tasks with checkboxes', icon: ListChecks, keywords: ['task', 'checkbox', 'todo', 'check'], run: (e, r) => e.chain().focus().deleteRange(r).toggleTaskList().run() },
  { title: 'Bulleted list', hint: 'A simple bullet list', icon: List, keywords: ['unordered', 'ul', 'bullet', 'point'], run: (e, r) => e.chain().focus().deleteRange(r).toggleBulletList().run() },
  { title: 'Ordered list', hint: 'A numbered list', icon: ListOrdered, keywords: ['ordered', 'ol', 'number'], run: (e, r) => e.chain().focus().deleteRange(r).toggleOrderedList().run() },
  { title: 'Quote', hint: 'Capture a quotation', icon: Quote, keywords: ['blockquote', 'citation'], run: (e, r) => e.chain().focus().deleteRange(r).toggleBlockquote().run() },
  { title: 'Code block', hint: 'Fenced code with highlighting', icon: SquareCode, keywords: ['code', 'pre', 'fence', 'snippet'], run: (e, r) => e.chain().focus().deleteRange(r).toggleCodeBlock().run() },
  { title: 'Table', hint: '3×3 table with a header row', icon: TableIcon, keywords: ['grid', 'sheet', 'cells'], run: (e, r) => e.chain().focus().deleteRange(r).insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run() },
  { title: 'Divider', hint: 'A horizontal rule', icon: Minus, keywords: ['hr', 'rule', 'separator', 'line'], run: (e, r) => e.chain().focus().deleteRange(r).setHorizontalRule().run() },
  { title: 'Image', hint: 'Embed an image by URL', icon: ImageIcon, keywords: ['photo', 'picture', 'img'], run: async (e, r) => {
    const url = (await prompt({ title: 'Insert image', placeholder: 'https://…', confirmLabel: 'Insert' }))?.trim()
    const chain = e.chain().focus().deleteRange(r)
    if (url) chain.setImage({ src: url }).run()
    else chain.run()
  } },
]

function filterItems(query: string): SlashItem[] {
  const q = query.trim().toLowerCase()
  if (!q) return ITEMS
  return ITEMS.filter((i) => i.title.toLowerCase().includes(q) || i.keywords.some((k) => k.includes(q)))
}

interface MenuHandle {
  onKeyDown: (e: KeyboardEvent) => boolean
}

const SlashMenu = forwardRef<MenuHandle, { items: SlashItem[]; command: (item: SlashItem) => void }>(function SlashMenu({ items, command }, ref) {
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
      if (e.key === 'Enter') {
        if (items[active]) command(items[active])
        return true
      }
      return false
    },
  }))

  if (items.length === 0) {
    return <div className="w-64 rounded-xl border border-line bg-card p-3 text-xs text-muted shadow-lg">No blocks match.</div>
  }
  return (
    <div ref={listRef} className="max-h-72 w-64 overflow-y-auto rounded-xl border border-line bg-card p-1 shadow-lg">
      {items.map((item, i) => {
        const Icon = item.icon
        return (
          <button
            key={item.title}
            type="button"
            data-active={i === active}
            onMouseEnter={() => setActive(i)}
            onMouseDown={(e) => {
              e.preventDefault()
              command(item)
            }}
            className={cn('flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left', i === active ? 'bg-sidebar' : 'hover:bg-sidebar')}
          >
            <span className={cn('grid h-7 w-7 shrink-0 place-items-center rounded-md border border-line-subtle', i === active ? 'text-accent' : 'text-muted')}>
              <Icon size={15} />
            </span>
            <span className="min-w-0">
              <span className="block truncate text-xs font-medium text-fg">{item.title}</span>
              <span className="block truncate text-[11px] text-muted">{item.hint}</span>
            </span>
          </button>
        )
      })}
    </div>
  )
})

// Position a fixed popup at the caret rect, flipping above if it would overflow.
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

const suggestion: Omit<SuggestionOptions<SlashItem>, 'editor'> = {
  char: '/',
  allowSpaces: false,
  startOfLine: false,
  items: ({ query }) => filterItems(query),
  command: ({ editor, range, props }) => props.run(editor, range),
  render: () => {
    let component: ReactRenderer<MenuHandle> | null = null
    let popup: HTMLDivElement | null = null
    return {
      onStart: (props) => {
        component = new ReactRenderer(SlashMenu, {
          props: { items: props.items, command: (item: SlashItem) => props.command(item) },
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
        component?.updateProps({ items: props.items, command: (item: SlashItem) => props.command(item) })
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

export const SlashCommands = Extension.create({
  name: 'slashCommands',
  addProseMirrorPlugins() {
    return [Suggestion({ editor: this.editor, ...suggestion })]
  },
})
