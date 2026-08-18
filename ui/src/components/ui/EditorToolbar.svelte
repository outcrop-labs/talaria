<script lang="ts">
  import type { Readable } from 'svelte/store'
  import type { Editor } from 'svelte-tiptap'
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
    type LucideIcon as IconType,
  } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import Modal from './Modal.svelte'
  import Input from './Input.svelte'
  import Button from './Button.svelte'
  import DocLinkPopover from './DocLinkPopover.svelte'
  import type { DocSearchFn } from './rich-editor'

  // RichEditor's formatting toolbar (internal — RichEditor.svelte renders it).
  let {
    editor,
    onSubmit,
    docSearch,
  }: {
    editor: Readable<Editor>
    onSubmit?: () => void
    docSearch?: DocSearchFn
  } = $props()

  // Active state per mark/node. The svelte-tiptap store emits on every
  // transaction, so this recomputes per keystroke — the runes equivalent of
  // the React version's useEditorState selector.
  const s = $derived.by(() => {
    const e = $editor
    return {
      bold: e?.isActive('bold') ?? false,
      italic: e?.isActive('italic') ?? false,
      strike: e?.isActive('strike') ?? false,
      h1: e?.isActive('heading', { level: 1 }) ?? false,
      h2: e?.isActive('heading', { level: 2 }) ?? false,
      h3: e?.isActive('heading', { level: 3 }) ?? false,
      bullet: e?.isActive('bulletList') ?? false,
      ordered: e?.isActive('orderedList') ?? false,
      task: e?.isActive('taskList') ?? false,
      table: e?.isActive('table') ?? false,
      quote: e?.isActive('blockquote') ?? false,
      code: e?.isActive('code') ?? false,
      codeBlock: e?.isActive('codeBlock') ?? false,
      link: e?.isActive('link') ?? false,
    }
  })

  let linkOpen = $state(false)
  let linkUrl = $state('')
  let imgOpen = $state(false)
  let imgUrl = $state('')
  let docLinkOpen = $state(false)
  let docLinkAnchor = $state<HTMLSpanElement | null>(null)

  const insertDocLink = (doc: { title: string; icon?: string | null; href: string }) => {
    const label = `${doc.icon ? doc.icon + ' ' : ''}${doc.title}`
    const chain = $editor.chain().focus()
    if ($editor.state.selection.empty) {
      chain.insertContent({ type: 'text', text: label, marks: [{ type: 'link', attrs: { href: doc.href } }] }).run()
    } else {
      chain.setLink({ href: doc.href }).run()
    }
    docLinkOpen = false
  }

  const openLinkModal = () => {
    linkUrl = ($editor.getAttributes('link').href as string | undefined) ?? ''
    linkOpen = true
  }
  const applyLink = () => {
    const url = linkUrl.trim()
    const chain = $editor.chain().focus()
    if (!url) chain.unsetLink().run()
    else if ($editor.state.selection.empty)
      chain.insertContent({ type: 'text', text: url, marks: [{ type: 'link', attrs: { href: url } }] }).run()
    else chain.setLink({ href: url }).run()
    linkOpen = false
    linkUrl = ''
  }
  const applyImage = () => {
    const url = imgUrl.trim()
    if (url) $editor.chain().focus().setImage({ src: url }).run()
    imgOpen = false
    imgUrl = ''
  }

  const c = () => $editor.chain().focus()
</script>

{#snippet btn(icon: IconType, title: string, active: boolean, action: () => void)}
  {@const Icon = icon}
  <button
    type="button"
    {title}
    onmousedown={(e) => e.preventDefault()}
    onclick={action}
    class={cn(
      'grid h-7 w-7 place-items-center rounded-md transition-colors',
      active ? 'bg-raised text-accent' : 'text-muted dither-fill hover:text-fg',
      focusGold,
    )}
  >
    <Icon size={16} strokeWidth={2} />
  </button>
{/snippet}

<!-- Compact labeled buttons for the table segment — text beats cryptic icons
     for structural ops. -->
{#snippet tableBtn(label: string, title: string, action: () => void)}
  <button
    type="button"
    {title}
    onmousedown={(e) => {
      e.preventDefault()
      action()
    }}
    class={cn(
      'rounded px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors dither-fill hover:text-fg',
      focusGold,
    )}
  >
    {label}
  </button>
{/snippet}

{#if $editor}
  <div class="flex flex-wrap items-center gap-0.5 border-b border-line-subtle px-2 py-1">
    <Modal open={linkOpen} onClose={() => (linkOpen = false)} title={s.link ? 'Edit link' : 'Add link'}>
      <form
        onsubmit={(e) => {
          e.preventDefault()
          applyLink()
        }}
      >
        <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">URL</label>
        <!-- svelte-ignore a11y_autofocus -->
        <Input autofocus bind:value={linkUrl} placeholder="https://example.com" class="w-full" />
        <p class="mt-2 text-xs text-muted">Leave empty and apply to remove the link.</p>
      </form>
      {#snippet footer()}
        <div class="flex justify-end gap-2">
          <Button variant="outline" size="sm" onclick={() => (linkOpen = false)}>Cancel</Button>
          <Button size="sm" onclick={applyLink}>{linkUrl.trim() ? 'Apply' : 'Remove'}</Button>
        </div>
      {/snippet}
    </Modal>
    <Modal open={imgOpen} onClose={() => (imgOpen = false)} title="Insert image">
      <form
        onsubmit={(e) => {
          e.preventDefault()
          applyImage()
        }}
      >
        <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Image URL</label>
        <!-- svelte-ignore a11y_autofocus -->
        <Input autofocus bind:value={imgUrl} placeholder="https:///image.png" class="w-full" />
      </form>
      {#snippet footer()}
        <div class="flex justify-end gap-2">
          <Button variant="outline" size="sm" onclick={() => (imgOpen = false)}>Cancel</Button>
          <Button size="sm" onclick={applyImage} disabled={!imgUrl.trim()}>Insert</Button>
        </div>
      {/snippet}
    </Modal>
    {@render btn(Bold, 'Bold', s.bold, () => c().toggleBold().run())}
    {@render btn(Italic, 'Italic', s.italic, () => c().toggleItalic().run())}
    {@render btn(Strikethrough, 'Strikethrough', s.strike, () => c().toggleStrike().run())}
    <span class="mx-1 h-4 w-px bg-line-subtle"></span>
    {@render btn(Heading1, 'Big heading', s.h1, () => c().toggleHeading({ level: 1 }).run())}
    {@render btn(Heading2, 'Medium heading', s.h2, () => c().toggleHeading({ level: 2 }).run())}
    {@render btn(Heading3, 'Small heading', s.h3, () => c().toggleHeading({ level: 3 }).run())}
    {@render btn(List, 'Bulleted list', s.bullet, () => c().toggleBulletList().run())}
    {@render btn(ListOrdered, 'Numbered list', s.ordered, () => c().toggleOrderedList().run())}
    {@render btn(ListChecks, 'Task list', s.task, () => c().toggleTaskList().run())}
    {@render btn(Quote, 'Quote', s.quote, () => c().toggleBlockquote().run())}
    {@render btn(Code, 'Inline code', s.code, () => c().toggleCode().run())}
    {@render btn(SquareCode, 'Code block', s.codeBlock, () => c().toggleCodeBlock().run())}
    <span class="mx-1 h-4 w-px bg-line-subtle"></span>
    {@render btn(TableIcon, s.table ? 'Delete table' : 'Insert table', s.table, () =>
      s.table ? c().deleteTable().run() : c().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run(),
    )}
    {#if s.table}
      <span class="mx-1 h-4 w-px bg-line-subtle"></span>
      {@render tableBtn('+ row', 'Add row below', () => c().addRowAfter().run())}
      {@render tableBtn('+ col', 'Add column right', () => c().addColumnAfter().run())}
      {@render tableBtn('− row', 'Delete this row', () => c().deleteRow().run())}
      {@render tableBtn('− col', 'Delete this column', () => c().deleteColumn().run())}
      {@render tableBtn('header', 'Toggle header row', () => c().toggleHeaderRow().run())}
    {/if}
    {@render btn(ImageIcon, 'Insert image', false, () => {
      imgUrl = ''
      imgOpen = true
    })}
    {@render btn(LinkIcon, 'Link', s.link, openLinkModal)}
    {#if docSearch}
      <!-- The span is the popover's anchor: the panel portals to <body>, so it
           positions from this rect rather than from a `relative` parent. -->
      <span bind:this={docLinkAnchor} class="relative">
        {@render btn(FileText, 'Link to a document', docLinkOpen, () => (docLinkOpen = !docLinkOpen))}
        {#if docLinkOpen}
          <DocLinkPopover anchor={docLinkAnchor} search={docSearch} onPick={insertDocLink} onClose={() => (docLinkOpen = false)} />
        {/if}
      </span>
    {/if}
    {#if onSubmit}
      <button
        type="button"
        title="Send (Ctrl+Enter)"
        onmousedown={(e) => e.preventDefault()}
        onclick={onSubmit}
        class={cn(
          // Primary button (spec §8): gold fill, dark ground glyph/label, mono
          // uppercase, radius 6.
          'ml-auto flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-surface transition-all hover:brightness-110',
          focusGold,
        )}
      >
        <SendHorizontal size={14} strokeWidth={2} />
        Send
      </button>
    {/if}
  </div>
{/if}
