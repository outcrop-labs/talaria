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
    Paperclip,
    ChevronDown,
  } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { useUiVoice } from '@/lib/ui-voice'
  import { focusGold } from '@/components/chat/chat-chrome'
  import Modal from './Modal.svelte'
  import Input from './Input.svelte'
  import Button from './Button.svelte'
  import IconButton from './IconButton.svelte'
  import DocLinkPopover from './DocLinkPopover.svelte'
  import DropdownMenu from './DropdownMenu.svelte'
  import type { ContextMenuEntry, MenuIcon } from './context-menu.svelte'
  import type { DocSearchFn } from './rich-editor'

  // RichEditor's formatting toolbar (internal — RichEditor.svelte renders it).
  let {
    editor,
    onSubmit,
    docSearch,
    onAttachFiles,
  }: {
    editor: Readable<Editor>
    onSubmit?: () => void
    docSearch?: DocSearchFn
    /** RichEditor passes its insertFile only when its `attachments` prop is
     *  on — the paperclip (file picker → insert at cursor) shows exactly there. */
    onAttachFiles?: (files: FileList) => void
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
  const toggleTable = () => (s.table ? c().deleteTable().run() : c().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run())
  const openImageModal = () => {
    imgUrl = ''
    imgOpen = true
  }

  let attachRef = $state<HTMLInputElement | null>(null)

  const pickAttach = (files: FileList | null) => {
    if (files?.length) onAttachFiles?.(files)
    // Reset so picking the same file again still fires onchange —
    // AttachButton.svelte's hidden-input pattern.
    if (attachRef) attachRef.value = ''
  }

  // The link and image dialogs follow the voice of the page the editor sits on.
  const sans = useUiVoice() === 'sans'
  const fieldLabel = sans
    ? 'mb-1.5 block font-sans text-[13px] font-medium leading-5 text-fg'
    : 'mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim' // voice-ok: the mono branch, as it was

  // The settings voice shows the five formats people reach for and keeps the
  // rest in two menus. A format that is on tints its icon, as on the strip.
  const mark = (icon: IconType, on = false): MenuIcon => [icon, { size: 16, class: on ? 'text-accent' : undefined }]
  const heading = $derived(s.h1 || s.h2 || s.h3)
  const styleItems = (): ContextMenuEntry[] => [
    { label: 'Paragraph', checked: !heading, onSelect: () => c().setParagraph().run() },
    { label: 'Big heading', checked: s.h1, onSelect: () => c().setHeading({ level: 1 }).run() },
    { label: 'Medium heading', checked: s.h2, onSelect: () => c().setHeading({ level: 2 }).run() },
    { label: 'Small heading', checked: s.h3, onSelect: () => c().setHeading({ level: 3 }).run() },
  ]
  const moreItems = (): ContextMenuEntry[] => [
    { label: 'Strikethrough', icon: mark(Strikethrough, s.strike), onSelect: () => c().toggleStrike().run() },
    { label: 'Task list', icon: mark(ListChecks, s.task), onSelect: () => c().toggleTaskList().run() },
    { label: 'Quote', icon: mark(Quote, s.quote), onSelect: () => c().toggleBlockquote().run() },
    { label: 'Inline code', icon: mark(Code, s.code), onSelect: () => c().toggleCode().run() },
    { label: 'Code block', icon: mark(SquareCode, s.codeBlock), onSelect: () => c().toggleCodeBlock().run() },
    'sep',
    { label: s.table ? 'Delete table' : 'Insert table', icon: mark(TableIcon), onSelect: toggleTable },
    { label: 'Insert image', icon: mark(ImageIcon), onSelect: openImageModal },
    ...(onAttachFiles ? [{ label: 'Attach file', icon: mark(Paperclip), onSelect: () => attachRef?.click() }] : []),
    ...(docSearch ? [{ label: 'Link to a document', icon: mark(FileText), onSelect: () => (docLinkOpen = true) }] : []),
  ]
</script>

{#snippet btn(icon: IconType, title: string, active: boolean, action: () => void)}
  {@const Icon = icon}
  <!-- Active keeps its accent glyph (the editor's "this format is on" ink),
       not the primitive's default active text color. -->
  <IconButton
    size="sm"
    {title}
    {active}
    class={active ? 'text-accent' : ''}
    onmousedown={(e) => e.preventDefault()}
    onclick={action}
  >
    <Icon size={16} strokeWidth={2} />
  </IconButton>
{/snippet}

<!-- Compact labeled buttons for the table segment — text beats cryptic icons
     for structural ops. -->
{#snippet tableBtn(label: string, sansLabel: string, title: string, action: () => void)}
  <button
    type="button"
    {title}
    onmousedown={(e) => {
      e.preventDefault()
      action()
    }}
    class={cn(
      sans
        ? 'flex h-7 items-center rounded-md px-2 font-sans text-xs font-medium text-muted transition-colors hover:bg-hover hover:text-fg'
        : 'rounded px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors dither-fill hover:text-fg', // voice-ok: the mono branch, as it was
      focusGold,
    )}
  >
    {sans ? sansLabel : label}
  </button>
{/snippet}

{#snippet tableOps()}
  {@render tableBtn('+ row', 'Add row', 'Add row below', () => c().addRowAfter().run())}
  {@render tableBtn('+ col', 'Add column', 'Add column right', () => c().addColumnAfter().run())}
  {@render tableBtn('− row', 'Delete row', 'Delete this row', () => c().deleteRow().run())}
  {@render tableBtn('− col', 'Delete column', 'Delete this column', () => c().deleteColumn().run())}
  {@render tableBtn('header', 'Header row', 'Toggle header row', () => c().toggleHeaderRow().run())}
{/snippet}

<!-- A menu on the settings strip: a word and a chevron. With a format inside it on, the word takes the primary ink:
     gold is for the toggles, and a gold word reads as a link. -->
{#snippet menuBtn(label: string, title: string, open: boolean, active: boolean)}
  <button
    type="button"
    {title}
    aria-label={title}
    aria-haspopup="menu"
    aria-expanded={open}
    onmousedown={(e) => e.preventDefault()}
    class={cn(
      // relative: the menu wraps its trigger in an inline span, and a button that is not positioned sits under that
      // span in the hit order a test's click checks.
      'relative flex h-7 items-center gap-1 rounded-md px-2 font-sans text-xs font-medium transition-colors hover:bg-hover hover:text-fg',
      open ? 'bg-hover text-fg' : active ? 'text-fg' : 'text-muted',
      focusGold,
    )}
  >
    {label}<ChevronDown size={14} aria-hidden="true" />
  </button>
{/snippet}

{#if $editor}
<!-- A plain multi-file picker; upload + insert branch on each file's mime
     (RichEditor's insertFile) — the toolbar only picks. -->
<input bind:this={attachRef} type="file" multiple hidden onchange={(e) => pickAttach(e.currentTarget.files)} />
  <div class="flex flex-wrap items-center gap-0.5 border-b border-line-subtle px-2 py-1">
    <Modal open={linkOpen} onClose={() => (linkOpen = false)} title={s.link ? 'Edit link' : 'Add link'}>
      <form
        onsubmit={(e) => {
          e.preventDefault()
          applyLink()
        }}
      >
        <label for="et-link-url" class={fieldLabel}>URL</label>
        <!-- svelte-ignore a11y_autofocus -->
        <Input id="et-link-url" autofocus bind:value={linkUrl} placeholder="https://example.com" class="w-full" />
        <p class={sans ? 'mt-1.5 font-sans text-[13px] leading-5 text-muted' : 'mt-2 text-xs text-muted'}>Leave empty and apply to remove the link.</p>
      </form>
      {#snippet footer()}
        <div class="flex justify-end gap-2">
          <Button variant={sans ? 'ghost' : 'outline'} size="sm" onclick={() => (linkOpen = false)}>Cancel</Button>
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
        <label for="et-img-url" class={fieldLabel}>Image URL</label>
        <!-- svelte-ignore a11y_autofocus -->
        <Input id="et-img-url" autofocus bind:value={imgUrl} placeholder="https:///image.png" class="w-full" />
      </form>
      {#snippet footer()}
        <div class="flex justify-end gap-2">
          <Button variant={sans ? 'ghost' : 'outline'} size="sm" onclick={() => (imgOpen = false)}>Cancel</Button>
          <Button size="sm" onclick={applyImage} disabled={!imgUrl.trim()}>Insert</Button>
        </div>
      {/snippet}
    </Modal>
    {#if sans}
      {@render btn(Bold, 'Bold', s.bold, () => c().toggleBold().run())}
      {@render btn(Italic, 'Italic', s.italic, () => c().toggleItalic().run())}
      <span class="mx-1 h-4 w-px bg-line-subtle"></span>
      {@render btn(List, 'Bulleted list', s.bullet, () => c().toggleBulletList().run())}
      {@render btn(ListOrdered, 'Numbered list', s.ordered, () => c().toggleOrderedList().run())}
      {@render btn(LinkIcon, 'Link', s.link, openLinkModal)}
      <span class="mx-1 h-4 w-px bg-line-subtle"></span>
      <DropdownMenu items={styleItems} align="left">
        {#snippet trigger(open)}{@render menuBtn('Text style', 'Text style', open, heading)}{/snippet}
      </DropdownMenu>
      <!-- The span is the document-link popover's anchor, as in the mono strip. -->
      <span bind:this={docLinkAnchor} class="relative">
        <DropdownMenu items={moreItems} align="left">
          {#snippet trigger(open)}{@render menuBtn('More', 'More formatting', open, s.strike || s.task || s.quote || s.code || s.codeBlock)}{/snippet}
        </DropdownMenu>
        {#if docLinkOpen && docSearch}
          <DocLinkPopover anchor={docLinkAnchor} search={docSearch} onPick={insertDocLink} onClose={() => (docLinkOpen = false)} />
        {/if}
      </span>
      {#if s.table}
        <span class="mx-1 h-4 w-px bg-line-subtle"></span>
        {@render tableOps()}
      {/if}
    {:else}
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
    {@render btn(TableIcon, s.table ? 'Delete table' : 'Insert table', s.table, toggleTable)}
    {#if s.table}
      <span class="mx-1 h-4 w-px bg-line-subtle"></span>
      {@render tableOps()}
    {/if}
    {@render btn(ImageIcon, 'Insert image', false, openImageModal)}
    {#if onAttachFiles}
      {@render btn(Paperclip, 'Attach file', false, () => attachRef?.click())}
    {/if}
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
    {/if}
    {#if onSubmit && sans}
      <Button size="xs" class="ml-auto gap-1.5" title="Send (Ctrl+Enter)" onmousedown={(e) => e.preventDefault()} onclick={onSubmit}>
        <SendHorizontal size={14} strokeWidth={2} />
        Send
      </Button>
    {:else if onSubmit}
      <button
        type="button"
        title="Send (Ctrl+Enter)"
        onmousedown={(e) => e.preventDefault()}
        onclick={onSubmit}
        class={cn(
          // Primary button (spec §8): gold fill, dark ground glyph/label, mono
          // uppercase, radius 6.
          'ml-auto flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1.5 font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-surface transition-all hover:brightness-110', // voice-ok: the mono branch, as it was
          focusGold,
        )}
      >
        <SendHorizontal size={14} strokeWidth={2} />
        Send
      </button>
    {/if}
  </div>
{/if}
