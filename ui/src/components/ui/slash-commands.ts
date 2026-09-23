import { placeMenu } from '@/lib/menu-position'
import { flushSync, mount, unmount } from 'svelte'
import { Extension, type Editor, type Range } from '@tiptap/core'
import Suggestion, { type SuggestionOptions } from '@tiptap/suggestion'
import { PluginKey } from '@tiptap/pm/state'
import {
  Heading1, Heading2, Heading3, List, ListOrdered, ListChecks, Quote,
  SquareCode, Table as TableIcon, Image as ImageIcon, Minus, Type, Paperclip,
  type LucideIcon as IconType,
} from '@lucide/svelte'
import SlashMenu from './SlashMenu.svelte'
import { prompt } from './confirm.svelte'

// A slash-command menu (like Outline's block menu): type "/" to insert a block.
// Filterable, keyboard-navigable, positioned at the caret. Built on TipTap's

export interface SlashItem {
  title: string
  hint: string
  icon: IconType
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
    const url = (await prompt({ title: 'Insert image', placeholder: 'https://', confirmLabel: 'Insert' }))?.trim()
    const chain = e.chain().focus().deleteRange(r)
    if (url) chain.setImage({ src: url }).run()
    else chain.run()
  } },
  { title: 'Attach file', hint: 'Upload a file inline at the cursor', icon: Paperclip, keywords: ['file', 'attach', 'upload', 'attachment', 'pdf'], run: (e, r) => {
    // The menu closes and the range is claimed synchronously; the upload
    // lands at the caret when it finishes (TALA-2 inline attachments).
    e.chain().focus().deleteRange(r).run()
    const input = document.createElement('input')
    input.type = 'file'
    input.onchange = () => {
      const f = input.files?.[0]
      if (!f) return
      void import('@/lib/attachments').then(({ uploadFile, humanSize }) =>
        uploadFile(f).then((res) => {
          if (!('id' in res)) return
          const name = f.name.replace(/[\[\]]/g, '\\$1')
          e.chain().focus().insertContent(`[${name}|${humanSize(res.size)}](upload:${res.id}) `).run()
        }),
      )
    }
    input.click()
  } },
]

function filterItems(query: string): SlashItem[] {
  const q = query.trim().toLowerCase()
  if (!q) return ITEMS
  return ITEMS.filter((i) => i.title.toLowerCase().includes(q) || i.keywords.some((k) => k.includes(q)))
}

/** The imperative surface SlashMenu.svelte exports (mount() returns it). */
interface MenuHandle {
  update: (items: SlashItem[], command: (item: SlashItem) => void) => void
  onKeyDown: (e: KeyboardEvent) => boolean
}



const suggestion: Omit<SuggestionOptions<SlashItem>, 'editor'> = {
  char: '/',
  allowSpaces: false,
  startOfLine: false,
  items: ({ query }) => filterItems(query),
  command: ({ editor, range, props }) => props.run(editor, range),
  render: () => {
    let menu: MenuHandle | null = null
    let popup: HTMLDivElement | null = null
    return {
      onStart: (props) => {
        if (!props.clientRect) return
        popup = document.createElement('div')
        popup.style.position = 'fixed'
        popup.style.zIndex = '60'
        document.body.appendChild(popup)
        menu = mount(SlashMenu, { target: popup }) as unknown as MenuHandle
        menu.update(props.items, (item: SlashItem) => props.command(item))
        // Render synchronously so placeMenu() can measure the popup's height.
        flushSync()
        const rect = props.clientRect()
        if (rect) placeMenu(popup, rect)
      },
      onUpdate: (props) => {
        menu?.update(props.items, (item: SlashItem) => props.command(item))
        flushSync()
        const rect = props.clientRect?.()
        if (popup && rect) placeMenu(popup, rect)
      },
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

export const SlashCommands = Extension.create({
  name: 'slashCommands',
  addProseMirrorPlugins() {
    return [Suggestion({ editor: this.editor, pluginKey: new PluginKey('slashCommands'), ...suggestion })]
  },
})
