<script lang="ts">
  import { onMount } from 'svelte'
  import type { Snippet } from 'svelte'
  import type { Readable } from 'svelte/store'
  import { createEditor, EditorContent, type Editor } from 'svelte-tiptap'
  import StarterKit from '@tiptap/starter-kit'
  import Link from '@tiptap/extension-link'
  import Placeholder from '@tiptap/extension-placeholder'
  import { Markdown } from 'tiptap-markdown'
  import { Extension } from '@tiptap/core'
  import { MentionSuggest } from '@/components/ui/mention-suggest'
  import { EmojiSuggest } from '@/components/chat/emoji-suggest'
  import SendButton from '@/components/chat/SendButton.svelte'
  import StopButton from '@/components/chat/StopButton.svelte'
  import ComposerToolbar from './ComposerToolbar.svelte'
  import { emojify } from '@/lib/emoji'
  import type { Mentionable } from '@/components/chat/mentions.svelte'

  // The Slack-shaped message editor: rich formatting with markdown under the
  // hood. Type syntax (**bold**, `code`, ``` blocks, > quotes, - lists) or use
  // the toolbar; @ mentions and : emoji autocomplete inline. Enter sends —
  // except inside a code block, where it makes a newline (Slack semantics);
  // Shift+Enter is always a soft newline. Messages travel as markdown, which is
  // exactly what the message list renders and agents read.
  //
  // Mercury anatomy (spec §7, updated): the editor sits in an INSET prompt well
  // — ground background, hairline border, radius 6, 14px padding all round —
  // and NOTHING else; the gold 36×36 send tile is the LAST item on the 36px
  // control rail below, outside the well, every other control before it. The
  // host supplies the outer #141312 panel (strong border, radius 8, padding 8)
  // so attachment chips can ride between well and rail.

  let {
    placeholder,
    mentionables,
    onSubmit,
    onFiles,
    onEscape,
    onEmptyChange,
    disabled,
    canSend,
    onStop,
    leftControls,
    rightControls,
    controlRail,
    compactOnNarrow,
  }: {
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
    /** THE SEND TILE BECOMES THE STOP TILE while this is set: the host passes
     *  its stop handler for exactly as long as a reply is streaming, and the
     *  rail's last item swaps the gold ArrowUp for the pulsing stop square —
     *  one tile, two jobs, never both at once. Enter still submits (the host
     *  queues the message into the streaming turn) and Escape still reaches
     *  `onEscape`. */
    onStop?: () => void
    /** Host controls rendered at the START of the bottom row (attach, emoji). */
    leftControls?: Snippet
    /** Host controls rendered at the END of the bottom row (send hint, tiers, stop). */
    rightControls?: Snippet
    /** Complete replacement for the bottom rail. Used by surfaces whose
     *  control order is part of the product contract (the Inbox composer, for one). */
    controlRail?: Snippet
    /** Dense docks may hide the formatting strip below the sm breakpoint. */
    compactOnNarrow?: boolean
  } = $props()

  let empty = $state(true)
  let editor = $state() as Readable<Editor>

  const submit = () => {
    const e = $editor
    if (!e || e.isEmpty) {
      onSubmit('')
      return
    }
    const md = emojify((e.storage.markdown as { getMarkdown: () => string }).getMarkdown().trim())
    onSubmit(md)
  }

  // The keymap is bound once at editor creation; props in runes mode are live
  // bindings, so the closures below always see the fresh handlers (React
  // needed submitRef/escapeRef for this).
  const SendKeymap = Extension.create({
    name: 'chatSendKeymap',
    addKeyboardShortcuts() {
      return {
        Enter: () => {
          // Inside a code block Enter writes code; everywhere else it sends.
          if (this.editor.isActive('codeBlock')) return false
          submit()
          return true
        },
        'Shift-Enter': () =>
          this.editor.commands.first(({ commands }) => [
            () => commands.createParagraphNear(),
            () => commands.liftEmptyBlock(),
            () => commands.splitBlock(),
          ]),
        Escape: () => {
          if (onEscape) {
            onEscape()
            return true
          }
          return false
        },
      }
    },
  })

  onMount(() => {
    // Canonical svelte-tiptap setup (see RichEditor.svelte): the editor is
    // created onMount (SSR-safe) and exposed as a readable store that emits on
    // every transaction. (React's immediatelyRender/shouldRerenderOnTransaction
    // knobs are @tiptap/react concepts with no svelte-tiptap equivalent.)
    editor = createEditor({
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
        // 14px padding all round, min-height ~76px (spec §7). The well holds
        // ONLY the text — the send tile lives on the rail below.
        attributes: {
          class: 'tiptap max-h-40 overflow-y-auto px-3.5 py-3.5 font-sans text-sm leading-5',
          style: 'min-height:4.625rem',
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
        empty = editor.isEmpty
        onEmptyChange?.(editor.isEmpty)
      },
    })
  })

  // `disabled` can flip after mount; keep the live editor in sync. Guarded so
  // the resulting transaction doesn't re-trigger us forever.
  $effect(() => {
    const e = $editor
    if (e && e.isEditable !== !disabled) e.setEditable(!disabled)
  })

  // ── Imperative handle (ChatComposerHandle, see ./chat-composer) ────────────
  export function focus(): void {
    $editor?.chain().focus().run()
  }
  export function insertText(text: string): void {
    $editor?.chain().focus().insertContent(text).run()
  }
  export function isEmpty(): boolean {
    return $editor?.isEmpty ?? true
  }
  export function clear(): void {
    $editor?.commands.clearContent(true)
  }

  // Slack's split, Mercury's skin: the inset prompt well (text only) on top;
  // the gold send tile is the LAST item on the 36px chip rail below, with
  // host controls — attach, emoji, formatting, pickers — all before it. While
  // a reply streams, the SAME tile is the stop square (see `onStop`).
  const sendEnabled = $derived(!disabled && (canSend ?? !empty))
</script>

<div class="flex min-w-0 flex-1 flex-col gap-2">
  <div class="rounded-md border border-line bg-surface">
    <EditorContent editor={$editor} />
  </div>
  {#if controlRail}
    <div class="flex h-10 min-w-0 items-center gap-1.5 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {@render controlRail()}
      {#if onStop}
        <StopButton onClick={onStop} />
      {:else}
        <SendButton enabled={sendEnabled} onClick={submit} />
      {/if}
    </div>
  {:else}
    <!-- Overflow guard: a crowded rail on a narrow pane (the plan's split)
         scrolls instead of pushing the send tile past the panel's edge. -->
    <div class="flex h-10 min-w-0 items-center gap-1.5 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {@render leftControls?.()}
      {#if leftControls}<span class="mx-1 h-5 border-l border-line"></span>{/if}
      {#if editor}
        <ComposerToolbar {editor} class={compactOnNarrow ? 'hidden sm:flex' : undefined} />
      {/if}
      <span class="flex-1"></span>
      {@render rightControls?.()}
      {#if onStop}
        <StopButton onClick={onStop} />
      {:else}
        <SendButton enabled={sendEnabled} onClick={submit} />
      {/if}
    </div>
  {/if}
</div>
