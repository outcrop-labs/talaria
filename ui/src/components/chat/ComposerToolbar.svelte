<script lang="ts">
  import type { Readable } from 'svelte/store'
  import type { Editor } from 'svelte-tiptap'
  import {
    Bold,
    Code,
    Italic,
    List,
    ListOrdered,
    SquareCode,
    Strikethrough,
    TextQuote,
    type LucideIcon as IconType,
  } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'

  // Slack's little formatting row, under the input (internal —
  // ChatComposer.svelte renders it). Buttons toggle marks on the selection;
  // everything they do is also typeable as markdown.
  let { editor, class: className }: { editor: Readable<Editor>; class?: string } = $props()

  // Active state per mark/node. The svelte-tiptap store emits on every
  // transaction, so this recomputes per keystroke — the runes equivalent of
  // the React version's useEditorState selector.
  const active = $derived.by(() => {
    const e = $editor
    return {
      bold: e?.isActive('bold') ?? false,
      italic: e?.isActive('italic') ?? false,
      strike: e?.isActive('strike') ?? false,
      code: e?.isActive('code') ?? false,
      codeBlock: e?.isActive('codeBlock') ?? false,
      blockquote: e?.isActive('blockquote') ?? false,
      bulletList: e?.isActive('bulletList') ?? false,
      orderedList: e?.isActive('orderedList') ?? false,
    }
  })

  const c = () => $editor.chain().focus()
</script>

{#snippet btn(on: boolean, title: string, action: () => void, Icon: IconType)}
  <button
    type="button"
    {title}
    onmousedown={(e) => {
      e.preventDefault()
      action()
    }}
    class={cn(
      'grid h-7 w-7 place-items-center rounded-md transition-colors',
      on ? 'bg-raised text-fg' : 'text-muted hover:dither-fill hover:text-fg',
      focusGold,
    )}
  >
    <Icon size={13} />
  </button>
{/snippet}

<div class={cn('flex items-center gap-0.5', className)}>
  {@render btn(active.bold, 'Bold (⌘B or **text**)', () => c().toggleBold().run(), Bold)}
  {@render btn(active.italic, 'Italic (⌘I or *text*)', () => c().toggleItalic().run(), Italic)}
  {@render btn(active.strike, 'Strikethrough (~~text~~)', () => c().toggleStrike().run(), Strikethrough)}
  <span class="mx-1 h-5 border-l border-line"></span>
  {@render btn(active.code, 'Inline code (`code`)', () => c().toggleCode().run(), Code)}
  {@render btn(active.codeBlock, 'Code block (```)', () => c().toggleCodeBlock().run(), SquareCode)}
  <span class="mx-1 h-5 border-l border-line"></span>
  {@render btn(active.bulletList, 'Bulleted list (- item)', () => c().toggleBulletList().run(), List)}
  {@render btn(active.orderedList, 'Numbered list (1. item)', () => c().toggleOrderedList().run(), ListOrdered)}
  {@render btn(active.blockquote, 'Quote (> text)', () => c().toggleBlockquote().run(), TextQuote)}
</div>
