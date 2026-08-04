import { forwardRef, useImperativeHandle, useRef, useState } from 'react'
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
import { SendButton } from '@/components/chat/composer-buttons'
import { focusGold } from '@/components/chat/chat-chrome'
import { emojify } from '@/lib/emoji'
import type { Mentionable } from '@/components/chat/mentions'

// The Slack-shaped message editor: rich formatting with markdown under the
// hood. Type syntax (**bold**, `code`, ``` blocks, > quotes, - lists) or use
// the toolbar; @ mentions and : emoji autocomplete inline. Enter sends —
// except inside a code block, where it makes a newline (Slack semantics);
// Shift+Enter is always a soft newline. Messages travel as markdown, which is
// exactly what the message list renders and agents read.
//
// Gentle dew anatomy (spec §7): the editor sits in an INSET prompt well —
// ground background, hairline border, radius 6 — with the gold 36×36 send
// tile pinned top-right; every other control lives on the 36px chip rail
// below. The host supplies the outer #141312 panel (strong border, radius 8,
// padding 8) so attachment chips can ride between well and rail.

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
    /** Host override for the send tile's enabled state (e.g. pending
     *  attachments make an empty editor sendable). Falls back to "editor has
     *  content" when omitted. */
    canSend?: boolean
    /** Host controls rendered at the START of the bottom row (attach, emoji). */
    leftControls?: React.ReactNode
    /** Host controls rendered at the END of the bottom row (send hint, tiers, stop). */
    rightControls?: React.ReactNode
    /** Dense docks may hide the formatting strip below the sm breakpoint. */
    compactOnNarrow?: boolean
  }
>(function ChatComposer({ placeholder, mentionables, onSubmit, onFiles, onEscape, onEmptyChange, disabled, canSend, leftControls, rightControls, compactOnNarrow }, ref) {
  const [empty, setEmpty] = useState(true)
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
      // Prompt well interior: transparent 14/20 sans on the ground inset,
      // 14px padding, min-height ~76px, right side clearing the 36px send
      // tile (spec §7).
      attributes: {
        class: 'tiptap max-h-40 overflow-y-auto py-3.5 pl-3.5 pr-14 font-sans text-sm leading-5',
        style: 'min-height:4.75rem',
      },
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
    onUpdate: ({ editor }) => {
      setEmpty(editor.isEmpty)
      onEmptyChange?.(editor.isEmpty)
    },
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

  // Slack's split, Gentle dew's skin: the inset prompt well (with the gold
  // send tile inside, top-right) on top; every other control — attach, emoji,
  // formatting, pickers, stop — on the 36px chip rail below.
  const sendEnabled = !disabled && (canSend ?? !empty)
  return (
    <div className="flex min-w-0 flex-1 flex-col gap-2">
      <div className="relative rounded-md border border-line bg-surface">
        <EditorContent editor={editor} />
        <SendButton className="absolute right-2 top-2" enabled={sendEnabled} onClick={() => submitRef.current()} />
      </div>
      <div className="flex min-w-0 items-center gap-1.5">
        {leftControls}
        {leftControls != null && <span className="mx-1 h-5 border-l border-line" />}
        {editor && <ComposerToolbar editor={editor} className={compactOnNarrow ? 'hidden sm:flex' : undefined} />}
        <span className="flex-1" />
        {rightControls}
      </div>
    </div>
  )
})

/** Slack's little formatting row, under the input. Buttons toggle marks on the
 *  selection; everything they do is also typeable as markdown. */
function ComposerToolbar({ editor, className }: { editor: Editor; className?: string }) {
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
        'grid h-7 w-7 place-items-center rounded-md transition-colors',
        on ? 'bg-raised text-fg' : 'text-muted hover:bg-hover hover:text-fg',
        focusGold,
      )}
    >
      {children}
    </button>
  )
  const c = () => editor.chain().focus()
  return (
    <div className={cn('flex items-center gap-0.5', className)}>
      <Btn on={active.bold} title="Bold (⌘B or **text**)" action={() => c().toggleBold().run()}>
        <Bold size={13} />
      </Btn>
      <Btn on={active.italic} title="Italic (⌘I or *text*)" action={() => c().toggleItalic().run()}>
        <Italic size={13} />
      </Btn>
      <Btn on={active.strike} title="Strikethrough (~~text~~)" action={() => c().toggleStrike().run()}>
        <Strikethrough size={13} />
      </Btn>
      <span className="mx-1 h-5 border-l border-line" />
      <Btn on={active.code} title="Inline code (`code`)" action={() => c().toggleCode().run()}>
        <Code size={13} />
      </Btn>
      <Btn on={active.codeBlock} title="Code block (```)" action={() => c().toggleCodeBlock().run()}>
        <SquareCode size={13} />
      </Btn>
      <span className="mx-1 h-5 border-l border-line" />
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
