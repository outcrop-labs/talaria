<script lang="ts" module>
  import { mergeAttributes } from '@tiptap/core'
  import Link from '@tiptap/extension-link'
  import { createLowlight, common } from 'lowlight'

  const lowlight = createLowlight(common)

  // Links carry a native title tooltip so you can see the URL on hover in the editor.
  const HoverLink = Link.extend({
    renderHTML({ HTMLAttributes }) {
      return ['a', mergeAttributes(this.options.HTMLAttributes, HTMLAttributes, { title: HTMLAttributes.href }), 0]
    },
  })
</script>

<script lang="ts">
  import { onMount } from 'svelte'
  import type { Readable } from 'svelte/store'
  import { createEditor, EditorContent, type Editor } from 'svelte-tiptap'
  import StarterKit from '@tiptap/starter-kit'
  import Placeholder from '@tiptap/extension-placeholder'
  import CodeBlockLowlight from '@tiptap/extension-code-block-lowlight'
  import TaskList from '@tiptap/extension-task-list'
  import TaskItem from '@tiptap/extension-task-item'
  import Table from '@tiptap/extension-table'
  import TableRow from '@tiptap/extension-table-row'
  import TableHeader from '@tiptap/extension-table-header'
  import TableCell from '@tiptap/extension-table-cell'
  import Image from '@tiptap/extension-image'
  import { Markdown } from 'tiptap-markdown'
  import { SlashCommands } from '@/components/ui/slash-commands'
  import { MentionSuggest } from '@/components/ui/mention-suggest'
  import { BlockEscape } from '@/components/ui/editor-behavior'
  import type { Mentionable } from '@/components/chat/mentions.svelte'
  import { cn } from '@/lib/cn'
  import EditorToolbar from './EditorToolbar.svelte'
  import type { DocSearchFn } from './rich-editor'

  // WYSIWYG editor for normies; markdown under the hood (agents write/read markdown
  // via the API). Canonical svelte-tiptap setup: the editor is created onMount
  // (SSR-safe) and exposed as a readable store that emits on every transaction —
  // the toolbar reads active state from it, this host component doesn't re-render
  // on keystrokes. `onSave` fires on blur only when the content actually changed.
  // (React's immediatelyRender/shouldRerenderOnTransaction knobs are @tiptap/react
  // concepts with no svelte-tiptap equivalent.)
  let {
    value,
    onSave,
    onSubmit,
    editable = true,
    placeholder,
    minHeight = '5rem',
    bare = false,
    fill = false,
    docSearch,
    slash = false,
    mentions,
    prose = false,
    autosave = false,
    class: className,
  }: {
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
    class?: string
  } = $props()

  let lastSaved = value
  let saveTimer: ReturnType<typeof setTimeout> | null = null

  let editor = $state() as Readable<Editor>

  const insertImageFile = async (file: File) => {
    const { uploadFile } = await import('@/lib/attachments')
    const r = await uploadFile(file)
    if ('id' in r) $editor?.chain().focus().setImage({ src: `/api/uploads/${r.id}`, alt: file.name }).run()
  }

  onMount(() => {
    editor = createEditor({
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
        lastSaved = editor.storage.markdown.getMarkdown()
      },
      onBlur: ({ editor }) => {
        if (saveTimer) clearTimeout(saveTimer)
        const md = editor.storage.markdown.getMarkdown()
        if (md === lastSaved) return
        lastSaved = md
        onSave?.(md)
      },
      onUpdate: ({ editor }) => {
        // Save as you type — debounced — so there's no Save button to remember.
        if (!autosave) return
        if (saveTimer) clearTimeout(saveTimer)
        saveTimer = setTimeout(() => {
          const md = editor.storage.markdown.getMarkdown()
          if (md === lastSaved) return
          lastSaved = md
          onSave?.(md)
        }, 700)
      },
    })
    // The editor itself is destroyed when the store's last subscriber (the
    // template below) unsubscribes on unmount — createEditor wires that up.
    return () => {
      if (saveTimer) clearTimeout(saveTimer)
    }
  })

  // `editable` can flip after mount (view/edit toggles); keep the live editor
  // in sync. Guarded so the resulting transaction doesn't re-trigger us forever.
  $effect(() => {
    const e = $editor
    if (e && e.isEditable !== editable) e.setEditable(editable)
  })

  // ── Imperative handle ──────────────────────────────────────────────────────
  // React exposed these through a forwardRef handle; here they are component
  // instance methods — grab the instance with `bind:this`, it satisfies the
  // RichEditorHandle interface (see ./rich-editor).
  export function getMarkdown(): string {
    return $editor?.storage.markdown.getMarkdown() ?? ''
  }
  export function clear(): void {
    $editor?.commands.clearContent()
    lastSaved = ''
  }
  /** Selected text (plain), '' when empty — context menus and inline Muse. */
  export function getSelectionText(): string {
    const e = $editor
    if (!e) return ''
    const { from, to } = e.state.selection
    return e.state.doc.textBetween(from, to, ' ')
  }
  /** Replace the current selection with markdown/text. */
  export function replaceSelection(content: string): void {
    $editor?.chain().focus().deleteSelection().insertContent(content).run()
  }
  /** Toggle an inline mark on the selection (context-menu formatting). */
  export function toggleMark(mark: 'bold' | 'italic' | 'strike' | 'code'): void {
    const e = $editor
    if (!e) return
    const c = e.chain().focus()
    if (mark === 'bold') c.toggleBold().run()
    else if (mark === 'italic') c.toggleItalic().run()
    else if (mark === 'strike') c.toggleStrike().run()
    else c.toggleCode().run()
  }
  /** True when the caret sits inside a table (gates table menu items). */
  export function isInTable(): boolean {
    return !!$editor?.isActive('table')
  }
  /** Table structure ops for context menus. */
  export function tableCommand(
    cmd: 'addRowBefore' | 'addRowAfter' | 'addColumnBefore' | 'addColumnAfter' | 'deleteRow' | 'deleteColumn' | 'deleteTable' | 'insertTable',
  ): void {
    const e = $editor
    if (!e) return
    const c = e.chain().focus()
    if (cmd === 'addRowBefore') c.addRowBefore().run()
    else if (cmd === 'addRowAfter') c.addRowAfter().run()
    else if (cmd === 'addColumnBefore') c.addColumnBefore().run()
    else if (cmd === 'addColumnAfter') c.addColumnAfter().run()
    else if (cmd === 'deleteRow') c.deleteRow().run()
    else if (cmd === 'deleteColumn') c.deleteColumn().run()
    else if (cmd === 'deleteTable') c.deleteTable().run()
    else c.insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run()
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class={cn(
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
  onkeydown={(e) => {
    // Ctrl/Cmd+Enter submits (desktop-leaning). Handled here so it works from
    // anywhere in the contenteditable.
    if (onSubmit && (e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault()
      onSubmit()
    }
  }}
>
  {#if editable}
    <EditorToolbar {editor} {onSubmit} {docSearch} />
  {/if}
  <EditorContent editor={$editor} class={fill ? 'min-h-0 flex-1 overflow-y-auto [&>.tiptap]:min-h-full' : undefined} />
</div>
