import { forwardRef, useImperativeHandle, useRef, useState } from 'react'
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
  Heading2,
  List,
  ListOrdered,
  ListChecks,
  Table as TableIcon,
  Image as ImageIcon,
  Quote,
  Code,
  SquareCode,
  Link as LinkIcon,
  SendHorizontal,
  type LucideIcon,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { Modal } from '@/components/ui/modal'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'

export interface RichEditorHandle {
  getMarkdown: () => string
  clear: () => void
}

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
  className?: string
}>(function RichEditor({ value, onSave, onSubmit, editable = true, placeholder, minHeight = '5rem', bare = false, fill = false, className }, ref) {
  const lastSaved = useRef<string>(value)

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
      Placeholder.configure({ placeholder: placeholder ?? '' }),
      Markdown.configure({ html: false, breaks: true, transformPastedText: true }),
    ],
    content: value,
    // min-height goes on the contenteditable itself (not a wrapper) so clicking
    // anywhere in the empty area focuses and places the caret.
    editorProps: { attributes: { class: 'tiptap px-3 py-2 text-sm', style: `min-height:${fill ? '100%' : minHeight}` } },
    onCreate: ({ editor }) => {
      lastSaved.current = editor.storage.markdown.getMarkdown()
    },
    onBlur: ({ editor }) => {
      const md = editor.storage.markdown.getMarkdown()
      if (md === lastSaved.current) return
      lastSaved.current = md
      onSave?.(md)
    },
  })

  useImperativeHandle(
    ref,
    () => ({
      getMarkdown: () => editor?.storage.markdown.getMarkdown() ?? '',
      clear: () => {
        editor?.commands.clearContent()
        lastSaved.current = ''
      },
    }),
    [editor],
  )

  return (
    <div
      className={cn(
        'overflow-hidden',
        fill && 'flex h-full min-h-0 flex-col',
        // Editors always carry the off-bronze-black input surface — never rely on
        // the background behind them. `bare` only drops the border/rounding.
        editable && 'bg-[var(--theme-input)]',
        !bare && cn('rounded-xl border', editable ? 'border-line' : 'border-transparent'),
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
      {editable && <Toolbar editor={editor} onSubmit={onSubmit} />}
      <EditorContent editor={editor} className={fill ? 'min-h-0 flex-1 overflow-y-auto [&>.tiptap]:min-h-full' : undefined} />
    </div>
  )
})

function Toolbar({ editor, onSubmit }: { editor: Editor | null; onSubmit?: () => void }) {
  const s = useEditorState({
    editor,
    selector: ({ editor }) => ({
      bold: editor?.isActive('bold') ?? false,
      italic: editor?.isActive('italic') ?? false,
      strike: editor?.isActive('strike') ?? false,
      h2: editor?.isActive('heading', { level: 2 }) ?? false,
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
  if (!editor || !s) return null

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
      className={cn('grid h-7 w-7 place-items-center rounded transition-colors', active ? 'bg-card2 text-accent' : 'text-muted hover:bg-card hover:text-fg')}
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
      <Btn icon={Heading2} title="Heading" active={s.h2} onClick={() => c().toggleHeading({ level: 2 }).run()} />
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
      <Btn icon={ImageIcon} title="Insert image" active={false} onClick={() => { setImgUrl(''); setImgOpen(true) }} />
      <Btn icon={LinkIcon} title="Link" active={s.link} onClick={openLinkModal} />
      {onSubmit && (
        <button
          type="button"
          title="Send (Ctrl+Enter)"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onSubmit}
          className="ml-auto flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-surface transition-all hover:brightness-110"
        >
          <SendHorizontal size={14} strokeWidth={2} />
          Send
        </button>
      )}
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
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">URL</label>
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
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Image URL</label>
        <Input autoFocus value={url} onChange={(e) => onUrl(e.target.value)} placeholder="https://…/image.png" className="w-full" />
      </form>
    </Modal>
  )
}
