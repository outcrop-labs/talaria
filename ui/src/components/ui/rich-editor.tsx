import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react'
import { useEditor, useEditorState, EditorContent, mergeAttributes, type Editor } from '@tiptap/react'
import StarterKit from '@tiptap/starter-kit'
import Link from '@tiptap/extension-link'
import Placeholder from '@tiptap/extension-placeholder'
import CodeBlockLowlight from '@tiptap/extension-code-block-lowlight'
import TaskList from '@tiptap/extension-task-list'
import TaskItem from '@tiptap/extension-task-item'
import Table from '@tiptap/extension-table'
import TableRow from '@tiptap/extension-table-row'
import TableHeader from '@tiptap/extension-table-header'
import TableCell from '@tiptap/extension-table-cell'
import Image from '@tiptap/extension-image'
import { createLowlight, common } from 'lowlight'
import { Markdown } from 'tiptap-markdown'
import { SlashCommands } from '@/components/ui/slash-commands'
import { MentionSuggest } from '@/components/ui/mention-suggest'
import { BlockEscape } from '@/components/ui/editor-behavior'
import type { Mentionable } from '@/components/chat/mentions'

const lowlight = createLowlight(common)

// Links carry a native title tooltip so you can see the URL on hover in the editor.
const HoverLink = Link.extend({
  renderHTML({ HTMLAttributes }) {
    return ['a', mergeAttributes(this.options.HTMLAttributes, HTMLAttributes, { title: HTMLAttributes.href }), 0]
  },
})
import {
  Bold,
  Italic,
  Strikethrough,
  Heading1,
  Heading2,
  Heading3,
  List,
  ListOrdered,
  ListChecks,
  Table as TableIcon,
  Image as ImageIcon,
  Quote,
  Code,
  SquareCode,
  Link as LinkIcon,
  FileText,
  SendHorizontal,
  type LucideIcon,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { focusGold, popPanel } from '@/components/chat/chat-chrome'
import { Modal } from '@/components/ui/modal'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'

export interface RichEditorHandle {
  getMarkdown: () => string
  clear: () => void
  /** Selected text (plain), '' when empty — context menus and inline Muse. */
  getSelectionText: () => string
  /** Replace the current selection with markdown/text. */
  replaceSelection: (content: string) => void
  /** Toggle an inline mark on the selection (context-menu formatting). */
  toggleMark: (mark: 'bold' | 'italic' | 'strike' | 'code') => void
  /** True when the caret sits inside a table (gates table menu items). */
  isInTable: () => boolean
  /** Table structure ops for context menus. */
  tableCommand: (
    cmd: 'addRowBefore' | 'addRowAfter' | 'addColumnBefore' | 'addColumnAfter' | 'deleteRow' | 'deleteColumn' | 'deleteTable' | 'insertTable',
  ) => void
}

/** Optional cross-reference search: given a query, return docs to link to.
 *  When provided, the toolbar gains a "link to doc" button with fuzzy search. */
export type DocSearchFn = (query: string) => Promise<Array<{ id: string; title: string; icon?: string | null; href: string }>>

// WYSIWYG editor for normies; markdown under the hood (agents write/read markdown
// via the API). Canonical TipTap setup: immediatelyRender:false (SSR-safe),
// shouldRerenderOnTransaction:false (editor host doesn't re-render on keystrokes),
// and the toolbar reads active state via useEditorState. `onSave` fires on blur
// only when the content actually changed.
export const RichEditor = forwardRef<RichEditorHandle, {
  value: string
  onSave?: (markdown: string) => void
  onSubmit?: () => void
  editable?: boolean
  placeholder?: string
  minHeight?: string
  /** Drop the surrounding box (border/bg/rounding) so the editor sits flush. */
  bare?: boolean
  /** Stretch to fill the parent's height (parent must have a definite height). */
  fill?: boolean
  /** Enable the "link to another doc" toolbar button (KB cross-references). */
  docSearch?: DocSearchFn
  /** Enable the "/" slash-command block menu (document editors). */
  slash?: boolean
  /** Enable "@" people-mention autocomplete with these candidates — pass the
   *  people a mention will actually notify (board/plan members, not the org). */
  mentions?: Mentionable[]
  /** Flush, page-like surface: no box/border, text wrapped to a comfortable
   *  centered measure. For full-panel document editors. */
  prose?: boolean
  /** Save as you type (debounced) instead of only on blur — no Save button. */
  autosave?: boolean
  className?: string
}>(function RichEditor({ value, onSave, onSubmit, editable = true, placeholder, minHeight = '5rem', bare = false, fill = false, docSearch, slash = false, mentions, prose = false, autosave = false, className }, ref) {
  const lastSaved = useRef<string>(value)
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const editor = useEditor({
    immediatelyRender: false,
    shouldRerenderOnTransaction: false,
    editable,
    extensions: [
      StarterKit.configure({ heading: { levels: [1, 2, 3] }, codeBlock: false }),
      CodeBlockLowlight.configure({ lowlight }),
      HoverLink.configure({ openOnClick: false, autolink: true }),
      TaskList,
      TaskItem.configure({ nested: true }),
      Table.configure({ resizable: true }),
      TableRow,
      TableHeader,
      TableCell,
      Image.configure({ inline: false, allowBase64: false }),
      Placeholder.configure({
        placeholder: ({ node }) =>
          slash && node.type.name === 'paragraph' ? (placeholder ? `${placeholder}  ·  type “/” for blocks` : 'Type “/” for blocks, or just write') : (placeholder ?? ''),
      }),
      Markdown.configure({ html: false, breaks: true, transformPastedText: true }),
      BlockEscape,
      ...(slash ? [SlashCommands] : []),
      ...(mentions?.length ? [MentionSuggest.configure({ items: mentions })] : []),
    ],
    content: value,
    // min-height goes on the contenteditable itself (not a wrapper) so clicking
    // anywhere in the empty area focuses and places the caret.
    editorProps: {
      attributes: { class: 'tiptap px-3 py-2 text-sm', style: `min-height:${fill ? '100%' : minHeight}` },
      // Images paste/drop straight in: upload → insert the served URL as an
      // image node (markdown round-trips it as ![alt](url)).
      handlePaste: (_view, event) => {
        const files = Array.from(event.clipboardData?.files ?? []).filter((f) => f.type.startsWith('image/'))
        if (files.length === 0) return false
        event.preventDefault()
        for (const f of files) void insertImageFile(f)
        return true
      },
      handleDrop: (_view, event) => {
        const files = Array.from(event.dataTransfer?.files ?? []).filter((f) => f.type.startsWith('image/'))
        if (files.length === 0) return false
        event.preventDefault()
        for (const f of files) void insertImageFile(f)
        return true
      },
    },
    onCreate: ({ editor }) => {
      lastSaved.current = editor.storage.markdown.getMarkdown()
    },
    onBlur: ({ editor }) => {
      if (saveTimer.current) clearTimeout(saveTimer.current)
      const md = editor.storage.markdown.getMarkdown()
      if (md === lastSaved.current) return
      lastSaved.current = md
      onSave?.(md)
    },
    onUpdate: ({ editor }) => {
      // Save as you type — debounced — so there's no Save button to remember.
      if (!autosave) return
      if (saveTimer.current) clearTimeout(saveTimer.current)
      saveTimer.current = setTimeout(() => {
        const md = editor.storage.markdown.getMarkdown()
        if (md === lastSaved.current) return
        lastSaved.current = md
        onSave?.(md)
      }, 700)
    },
  })

  useEffect(() => () => {
    if (saveTimer.current) clearTimeout(saveTimer.current)
  }, [])

  const insertImageFile = async (file: File) => {
    const { uploadFile } = await import('@/lib/attachments')
    const r = await uploadFile(file)
    if ('id' in r) editor?.chain().focus().setImage({ src: `/api/uploads/${r.id}`, alt: file.name }).run()
  }

  useImperativeHandle(
    ref,
    () => ({
      getMarkdown: () => editor?.storage.markdown.getMarkdown() ?? '',
      clear: () => {
        editor?.commands.clearContent()
        lastSaved.current = ''
      },
      getSelectionText: () => {
        if (!editor) return ''
        const { from, to } = editor.state.selection
        return editor.state.doc.textBetween(from, to, ' ')
      },
      replaceSelection: (content: string) => editor?.chain().focus().deleteSelection().insertContent(content).run(),
      toggleMark: (mark: 'bold' | 'italic' | 'strike' | 'code') => {
        if (!editor) return
        const c = editor.chain().focus()
        if (mark === 'bold') c.toggleBold().run()
        else if (mark === 'italic') c.toggleItalic().run()
        else if (mark === 'strike') c.toggleStrike().run()
        else c.toggleCode().run()
      },
      isInTable: () => !!editor?.isActive('table'),
      tableCommand: (cmd) => {
        if (!editor) return
        const c = editor.chain().focus()
        if (cmd === 'addRowBefore') c.addRowBefore().run()
        else if (cmd === 'addRowAfter') c.addRowAfter().run()
        else if (cmd === 'addColumnBefore') c.addColumnBefore().run()
        else if (cmd === 'addColumnAfter') c.addColumnAfter().run()
        else if (cmd === 'deleteRow') c.deleteRow().run()
        else if (cmd === 'deleteColumn') c.deleteColumn().run()
        else if (cmd === 'deleteTable') c.deleteTable().run()
        else c.insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run()
      },
    }),
    [editor],
  )

  return (
    <div
      className={cn(
        'overflow-hidden',
        fill && 'flex h-full min-h-0 flex-col',
        // Editors carry the raised input surface (spec §8: raised tile bg +
        // hairline + radius 6 + gold focus) so they never rely on the background
        // behind them — EXCEPT `prose` mode, a flush page-like document surface
        // (no box, no fill) that inherits the panel.
        !prose && editable && 'bg-[var(--theme-input)]',
        !bare &&
          !prose &&
          cn(
            'rounded-md border',
            editable
              ? 'border-line transition-colors focus-within:border-accent focus-within:ring-1 focus-within:ring-[var(--theme-accent-border)]'
              : 'border-transparent',
          ),
        prose && 're-prose',
        className,
      )}
      onKeyDown={(e) => {
        // Ctrl/Cmd+Enter submits (desktop-leaning). Handled here so it works from
        // anywhere in the contenteditable.
        if (onSubmit && (e.metaKey || e.ctrlKey) && e.key === 'Enter') {
          e.preventDefault()
          onSubmit()
        }
      }}
    >
      {editable && <Toolbar editor={editor} onSubmit={onSubmit} docSearch={docSearch} />}
      <EditorContent editor={editor} className={fill ? 'min-h-0 flex-1 overflow-y-auto [&>.tiptap]:min-h-full' : undefined} />
    </div>
  )
})

/** Compact labeled buttons for the table segment — text beats cryptic icons
 *  for structural ops. */
function TableBtn({ label, title, onClick }: { label: string; title: string; onClick: () => void }) {
  return (
    <button
      type="button"
      title={title}
      onMouseDown={(e) => {
        e.preventDefault()
        onClick()
      }}
      className={cn(
        'rounded px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:bg-hover hover:text-fg',
        focusGold,
      )}
    >
      {label}
    </button>
  )
}

function Toolbar({ editor, onSubmit, docSearch }: { editor: Editor | null; onSubmit?: () => void; docSearch?: DocSearchFn }) {
  const s = useEditorState({
    editor,
    selector: ({ editor }) => ({
      bold: editor?.isActive('bold') ?? false,
      italic: editor?.isActive('italic') ?? false,
      strike: editor?.isActive('strike') ?? false,
      h1: editor?.isActive('heading', { level: 1 }) ?? false,
      h2: editor?.isActive('heading', { level: 2 }) ?? false,
      h3: editor?.isActive('heading', { level: 3 }) ?? false,
      bullet: editor?.isActive('bulletList') ?? false,
      ordered: editor?.isActive('orderedList') ?? false,
      task: editor?.isActive('taskList') ?? false,
      table: editor?.isActive('table') ?? false,
      quote: editor?.isActive('blockquote') ?? false,
      code: editor?.isActive('code') ?? false,
      codeBlock: editor?.isActive('codeBlock') ?? false,
      link: editor?.isActive('link') ?? false,
    }),
  })
  const [linkOpen, setLinkOpen] = useState(false)
  const [linkUrl, setLinkUrl] = useState('')
  const [imgOpen, setImgOpen] = useState(false)
  const [imgUrl, setImgUrl] = useState('')
  const [docLinkOpen, setDocLinkOpen] = useState(false)
  if (!editor || !s) return null

  const insertDocLink = (doc: { title: string; icon?: string | null; href: string }) => {
    const label = `${doc.icon ? doc.icon + ' ' : ''}${doc.title}`
    const chain = editor.chain().focus()
    if (editor.state.selection.empty) {
      chain.insertContent({ type: 'text', text: label, marks: [{ type: 'link', attrs: { href: doc.href } }] }).run()
    } else {
      chain.setLink({ href: doc.href }).run()
    }
    setDocLinkOpen(false)
  }

  const openLinkModal = () => {
    setLinkUrl((editor.getAttributes('link').href as string | undefined) ?? '')
    setLinkOpen(true)
  }
  const applyLink = () => {
    const url = linkUrl.trim()
    const chain = editor.chain().focus()
    if (!url) chain.unsetLink().run()
    else if (editor.state.selection.empty)
      chain.insertContent({ type: 'text', text: url, marks: [{ type: 'link', attrs: { href: url } }] }).run()
    else chain.setLink({ href: url }).run()
    setLinkOpen(false)
    setLinkUrl('')
  }

  const Btn = ({ icon: Icon, active, onClick, title }: { icon: LucideIcon; active: boolean; onClick: () => void; title: string }) => (
    <button
      type="button"
      title={title}
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className={cn(
        'grid h-7 w-7 place-items-center rounded-md transition-colors',
        active ? 'bg-raised text-accent' : 'text-muted hover:bg-hover hover:text-fg',
        focusGold,
      )}
    >
      <Icon size={16} strokeWidth={2} />
    </button>
  )
  const c = () => editor.chain().focus()

  return (
    <div className="flex flex-wrap items-center gap-0.5 border-b border-line-subtle px-2 py-1">
      <LinkModal
        open={linkOpen}
        url={linkUrl}
        onUrl={setLinkUrl}
        onClose={() => setLinkOpen(false)}
        onApply={applyLink}
        editing={s.link}
      />
      <ImageModal
        open={imgOpen}
        url={imgUrl}
        onUrl={setImgUrl}
        onClose={() => setImgOpen(false)}
        onApply={() => {
          const url = imgUrl.trim()
          if (url) editor.chain().focus().setImage({ src: url }).run()
          setImgOpen(false)
          setImgUrl('')
        }}
      />
      <Btn icon={Bold} title="Bold" active={s.bold} onClick={() => c().toggleBold().run()} />
      <Btn icon={Italic} title="Italic" active={s.italic} onClick={() => c().toggleItalic().run()} />
      <Btn icon={Strikethrough} title="Strikethrough" active={s.strike} onClick={() => c().toggleStrike().run()} />
      <span className="mx-1 h-4 w-px bg-line-subtle" />
      <Btn icon={Heading1} title="Big heading" active={s.h1} onClick={() => c().toggleHeading({ level: 1 }).run()} />
      <Btn icon={Heading2} title="Medium heading" active={s.h2} onClick={() => c().toggleHeading({ level: 2 }).run()} />
      <Btn icon={Heading3} title="Small heading" active={s.h3} onClick={() => c().toggleHeading({ level: 3 }).run()} />
      <Btn icon={List} title="Bulleted list" active={s.bullet} onClick={() => c().toggleBulletList().run()} />
      <Btn icon={ListOrdered} title="Numbered list" active={s.ordered} onClick={() => c().toggleOrderedList().run()} />
      <Btn icon={ListChecks} title="Task list" active={s.task} onClick={() => c().toggleTaskList().run()} />
      <Btn icon={Quote} title="Quote" active={s.quote} onClick={() => c().toggleBlockquote().run()} />
      <Btn icon={Code} title="Inline code" active={s.code} onClick={() => c().toggleCode().run()} />
      <Btn icon={SquareCode} title="Code block" active={s.codeBlock} onClick={() => c().toggleCodeBlock().run()} />
      <span className="mx-1 h-4 w-px bg-line-subtle" />
      <Btn
        icon={TableIcon}
        title={s.table ? 'Delete table' : 'Insert table'}
        active={s.table}
        onClick={() => (s.table ? c().deleteTable().run() : c().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run())}
      />
      {s.table && (
        <>
          <span className="mx-1 h-4 w-px bg-line-subtle" />
          <TableBtn label="+ row" title="Add row below" onClick={() => c().addRowAfter().run()} />
          <TableBtn label="+ col" title="Add column right" onClick={() => c().addColumnAfter().run()} />
          <TableBtn label="− row" title="Delete this row" onClick={() => c().deleteRow().run()} />
          <TableBtn label="− col" title="Delete this column" onClick={() => c().deleteColumn().run()} />
          <TableBtn label="header" title="Toggle header row" onClick={() => c().toggleHeaderRow().run()} />
        </>
      )}
      <Btn icon={ImageIcon} title="Insert image" active={false} onClick={() => { setImgUrl(''); setImgOpen(true) }} />
      <Btn icon={LinkIcon} title="Link" active={s.link} onClick={openLinkModal} />
      {docSearch && (
        <span className="relative">
          <Btn icon={FileText} title="Link to a document" active={docLinkOpen} onClick={() => setDocLinkOpen((v) => !v)} />
          {docLinkOpen && <DocLinkPopover search={docSearch} onPick={insertDocLink} onClose={() => setDocLinkOpen(false)} />}
        </span>
      )}
      {onSubmit && (
        <button
          type="button"
          title="Send (Ctrl+Enter)"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onSubmit}
          className={cn(
            // Primary button (spec §8): gold fill, dark ground glyph/label, mono
            // uppercase, radius 6.
            'ml-auto flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-surface transition-all hover:brightness-110',
            focusGold,
          )}
        >
          <SendHorizontal size={14} strokeWidth={2} />
          Send
        </button>
      )}
    </div>
  )
}

// Fuzzy doc-picker popover for cross-references. Debounced search over the
// caller-provided function; click a result to insert a link.
function DocLinkPopover({
  search,
  onPick,
  onClose,
}: {
  search: DocSearchFn
  onPick: (doc: { title: string; icon?: string | null; href: string }) => void
  onClose: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [q, setQ] = useState('')
  const [results, setResults] = useState<Array<{ id: string; title: string; icon?: string | null; href: string }>>([])
  useEffect(() => {
    const h = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose()
    }
    document.addEventListener('mousedown', h)
    return () => document.removeEventListener('mousedown', h)
  }, [onClose])
  useEffect(() => {
    let live = true
    const id = setTimeout(() => {
      void search(q).then((r) => live && setResults(r))
    }, 160)
    return () => {
      live = false
      clearTimeout(id)
    }
  }, [q, search])
  return (
    <div ref={ref} className={cn('absolute left-0 top-full z-30 mt-1 w-72', popPanel)}>
      <Input autoFocus size="sm" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search documents" className="mb-1.5" />
      <div className="max-h-64 overflow-y-auto">
        {results.length === 0 ? (
          <div className="px-2 py-3 text-center text-xs text-muted">{q.trim() ? 'No matches.' : 'Type to search docs.'}</div>
        ) : (
          results.map((d) => (
            <button
              key={d.id}
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => onPick(d)}
              className="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-left font-sans text-[13px] text-muted transition-colors hover:bg-hover hover:text-fg"
            >
              <span>{d.icon ?? '📄'}</span>
              <span className="min-w-0 flex-1 truncate">{d.title}</span>
            </button>
          ))
        )}
      </div>
    </div>
  )
}

function LinkModal({
  open,
  url,
  onUrl,
  onClose,
  onApply,
  editing,
}: {
  open: boolean
  url: string
  onUrl: (v: string) => void
  onClose: () => void
  onApply: () => void
  editing: boolean
}) {
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={editing ? 'Edit link' : 'Add link'}
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="outline" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" onClick={onApply}>
            {url.trim() ? 'Apply' : 'Remove'}
          </Button>
        </div>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault()
          onApply()
        }}
      >
        <label className="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">URL</label>
        <Input
          autoFocus
          value={url}
          onChange={(e) => onUrl(e.target.value)}
          placeholder="https://example.com"
          className="w-full"
        />
        <p className="mt-2 text-xs text-muted">Leave empty and apply to remove the link.</p>
      </form>
    </Modal>
  )
}

function ImageModal({
  open,
  url,
  onUrl,
  onClose,
  onApply,
}: {
  open: boolean
  url: string
  onUrl: (v: string) => void
  onClose: () => void
  onApply: () => void
}) {
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Insert image"
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="outline" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" onClick={onApply} disabled={!url.trim()}>
            Insert
          </Button>
        </div>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault()
          onApply()
        }}
      >
        <label className="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Image URL</label>
        <Input autoFocus value={url} onChange={(e) => onUrl(e.target.value)} placeholder="https:///image.png" className="w-full" />
      </form>
    </Modal>
  )
}
