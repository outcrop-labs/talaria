import { forwardRef, useImperativeHandle, useRef } from 'react'
import { EditorContent, useEditor, useEditorState, type Editor } from '@tiptap/react'
import StarterKit from '@tiptap/starter-kit'
import Link from '@tiptap/extension-link'
import Placeholder from '@tiptap/extension-placeholder'
import { Markdown } from 'tiptap-markdown'
import { Extension } from '@tiptap/core'
import { Bold, Code, Italic, List, ListOrdered, SquareCode, Strikethrough, TextQuote } from 'lucide-react'
import { cn } from '@/lib/cn'
import { MentionSuggest } from '@/components/ui/mention-suggest'
import { EmojiSuggest } from '@/components/chat/emoji-suggest'
import { emojify } from '@/lib/emoji'
import type { Mentionable } from '@/components/chat/mentions'

// The Slack-shaped message editor: rich formatting with markdown under the
// hood. Type syntax (**bold**, `code`, ``` blocks, > quotes, - lists) or use
// the toolbar; @ mentions and : emoji autocomplete inline. Enter sends —
// except inside a code block, where it makes a newline (Slack semantics);
// Shift+Enter is always a soft newline. Messages travel as markdown, which is
// exactly what the message list renders and agents read.

export interface ChatComposerHandle {
  focus: () => void
  insertText: (text: string) => void
  isEmpty: () => boolean
  clear: () => void
}

export const ChatComposer = forwardRef<
  ChatComposerHandle,
  {
    placeholder: string
    mentionables?: Mentionable[]
    /** Fired with the message as markdown (already emojified). */
    onSubmit: (markdown: string) => void
    /** Pasted/dropped files — the host uploads and tracks pending chips. */
    onFiles?: (files: File[]) => void
    onEscape?: () => void
    /** Notifies emptiness changes so the host can drive its send hint. */
    onEmptyChange?: (empty: boolean) => void
    disabled?: boolean
    /** Host controls rendered at the START of the bottom row (attach, emoji). */
    leftControls?: React.ReactNode
    /** Host controls rendered at the END of the bottom row (send hint, tiers, stop). */
    rightControls?: React.ReactNode
  }
>(function ChatComposer({ placeholder, mentionables, onSubmit, onFiles, onEscape, onEmptyChange, disabled, leftControls, rightControls }, ref) {
  // Refs so the keymap (bound once at editor creation) sees fresh handlers.
  const submitRef = useRef<() => void>(() => {})
  const escapeRef = useRef<(() => void) | undefined>(onEscape)
  escapeRef.current = onEscape

  const SendKeymap = Extension.create({
    name: 'chatSendKeymap',
    addKeyboardShortcuts() {
      return {
        Enter: () => {
          // Inside a code block Enter writes code; everywhere else it sends.
          if (this.editor.isActive('codeBlock')) return false
          submitRef.current()
          return true
        },
        'Shift-Enter': () =>
          this.editor.commands.first(({ commands }) => [
            () => commands.createParagraphNear(),
            () => commands.liftEmptyBlock(),
            () => commands.splitBlock(),
          ]),
        Escape: () => {
          if (escapeRef.current) {
            escapeRef.current()
            return true
          }
          return false
        },
      }
    },
  })

  const editor = useEditor({
    immediatelyRender: false,
    shouldRerenderOnTransaction: false,
    editable: !disabled,
    extensions: [
      // Chat-sized vocabulary: no headings/hr — Slack messages don't have them.
      StarterKit.configure({ heading: false, horizontalRule: false }),
      Link.configure({ openOnClick: false, autolink: true }),
      Placeholder.configure({ placeholder }),
      Markdown.configure({ html: false, breaks: true, transformPastedText: true }),
      SendKeymap,
      ...(mentionables?.length ? [MentionSuggest.configure({ items: mentionables })] : []),
      EmojiSuggest,
    ],
    content: '',
    editorProps: {
      attributes: { class: 'tiptap max-h-40 overflow-y-auto px-3 py-2.5 text-sm', style: 'min-height:2.75rem' },
      handlePaste: (_view, event) => {
        const files = Array.from(event.clipboardData?.files ?? [])
        if (files.length === 0) return false
        event.preventDefault()
        onFiles?.(files)
        return true
      },
      handleDrop: (_view, event) => {
        const files = Array.from(event.dataTransfer?.files ?? [])
        if (files.length === 0) return false
        event.preventDefault()
        onFiles?.(files)
        return true
      },
    },
    onUpdate: ({ editor }) => onEmptyChange?.(editor.isEmpty),
  })

  submitRef.current = () => {
    if (!editor || editor.isEmpty) {
      onSubmit('')
      return
    }
    const md = emojify((editor.storage.markdown as { getMarkdown: () => string }).getMarkdown().trim())
    onSubmit(md)
  }

  useImperativeHandle(ref, () => ({
    focus: () => editor?.chain().focus().run(),
    insertText: (text) => editor?.chain().focus().insertContent(text).run(),
    isEmpty: () => editor?.isEmpty ?? true,
    clear: () => editor?.commands.clearContent(true),
  }))

  // Slack's split: the message area is the whole top half, every control —
  // attach, emoji, formatting, send affordances — lives on the bottom row.
  return (
    <div className="min-w-0 flex-1">
      <EditorContent editor={editor} />
      <div className="flex items-center gap-0.5 border-t border-line-subtle px-1.5 pb-0.5 pt-1">
        {leftControls}
        {leftControls != null && <span className="mx-1 h-4 border-l border-line-subtle" />}
        {editor && <ComposerToolbar editor={editor} />}
        <span className="flex-1" />
        {rightControls}
      </div>
    </div>
  )
})

/** Slack's little formatting row, under the input. Buttons toggle marks on the
 *  selection; everything they do is also typeable as markdown. */
function ComposerToolbar({ editor }: { editor: Editor }) {
  const active = useEditorState({
    editor,
    selector: ({ editor: e }) => ({
      bold: e.isActive('bold'),
      italic: e.isActive('italic'),
      strike: e.isActive('strike'),
      code: e.isActive('code'),
      codeBlock: e.isActive('codeBlock'),
      blockquote: e.isActive('blockquote'),
      bulletList: e.isActive('bulletList'),
      orderedList: e.isActive('orderedList'),
    }),
  })
  const Btn = ({
    on,
    title,
    action,
    children,
  }: {
    on: boolean
    title: string
    action: () => void
    children: React.ReactNode
  }) => (
    <button
      type="button"
      title={title}
      onMouseDown={(e) => {
        e.preventDefault()
        action()
      }}
      className={cn(
        'grid h-6 w-6 place-items-center rounded transition-colors',
        on ? 'bg-sidebar text-fg' : 'text-muted hover:bg-sidebar hover:text-fg',
      )}
    >
      {children}
    </button>
  )
  const c = () => editor.chain().focus()
  return (
    <div className="flex items-center gap-0.5">
      <Btn on={active.bold} title="Bold (⌘B or **text**)" action={() => c().toggleBold().run()}>
        <Bold size={13} />
      </Btn>
      <Btn on={active.italic} title="Italic (⌘I or *text*)" action={() => c().toggleItalic().run()}>
        <Italic size={13} />
      </Btn>
      <Btn on={active.strike} title="Strikethrough (~~text~~)" action={() => c().toggleStrike().run()}>
        <Strikethrough size={13} />
      </Btn>
      <span className="mx-1 h-4 border-l border-line-subtle" />
      <Btn on={active.code} title="Inline code (`code`)" action={() => c().toggleCode().run()}>
        <Code size={13} />
      </Btn>
      <Btn on={active.codeBlock} title="Code block (```)" action={() => c().toggleCodeBlock().run()}>
        <SquareCode size={13} />
      </Btn>
      <span className="mx-1 h-4 border-l border-line-subtle" />
      <Btn on={active.bulletList} title="Bulleted list (- item)" action={() => c().toggleBulletList().run()}>
        <List size={13} />
      </Btn>
      <Btn on={active.orderedList} title="Numbered list (1. item)" action={() => c().toggleOrderedList().run()}>
        <ListOrdered size={13} />
      </Btn>
      <Btn on={active.blockquote} title="Quote (> text)" action={() => c().toggleBlockquote().run()}>
        <TextQuote size={13} />
      </Btn>
    </div>
  )
}
