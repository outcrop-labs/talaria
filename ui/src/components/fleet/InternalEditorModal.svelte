<script lang="ts">
  import type { Snippet } from 'svelte'
  import { ChevronLeft } from '@lucide/svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import InternalEditor from '@/components/editor/InternalEditor.svelte'
  import type { MuseKind } from '@/lib/muse.svelte'
  import { fade, fly, QUICK } from '@/lib/motion'

  /** THE MODAL SHELL. The editor, version rail, streaming Muse and footer all
   *  live in `<InternalEditor>` now; this is just the chrome that wraps it —
   *  a centered modal by default, or a slide-in PANEL over the current dialog
   *  in `nested` mode (the ticket-editor pattern), which needs a positioned
   *  ancestor: the takeover Modal's panel provides one.
   *
   *  The same surface is embedded directly by the record views (Templates,
   *  Skills, Memory) through `<RecordEditor>`, so an agent-internal document
   *  no longer lives behind an Edit button. */
  let {
    open,
    onClose,
    title,
    subtitle,
    value,
    editable,
    saving,
    onSave,
    history,
    footerExtra,
    mode = 'rich',
    muse,
    nested = false,
  }: {
    open: boolean
    onClose: () => void
    title: string
    subtitle?: string
    value: string
    editable: boolean
    saving?: boolean
    onSave: (markdown: string) => Promise<void> | void
    /** Query params for /api/history (kind + owner/name or id). Omit to hide history. */
    history?: Record<string, string>
    /** Extra control on the left of the footer (e.g. a delete button). */
    footerExtra?: Snippet
    /** 'rich' (default) renders WYSIWYG markdown; 'plain' a mono text surface. */
    mode?: 'rich' | 'plain'
    /** Enable AI drafting for this document. */
    muse?: { kind: MuseKind; context?: string }
    /** Render as a slide-in panel over the CURRENT dialog instead of stacking a
     *  second modal. */
    nested?: boolean
  } = $props()
</script>

{#if nested}
  {#if open}
    <!-- framer-motion slid this in from the right (tween 180ms easeOut);
         fly with x travel and opacity pinned replicates the pure slide.
         |global: SkillEditor renders this with `open` already true at mount,
         so a local intro would be suppressed (ANIMATIONS.md). -->
    <div
      in:fly|global={{ x: '100%', duration: 180, opacity: 1 }}
      out:fade|global={QUICK}
      class="absolute inset-0 z-30 flex flex-col bg-[var(--theme-panel)]"
    >
      <div class="flex shrink-0 items-center gap-2 border-b border-line px-6 py-3.5">
        <IconButton size="sm" title="Back" onclick={onClose}>
          <ChevronLeft size={16} />
        </IconButton>
        <div class="text-sm font-semibold text-fg">{title}</div>
        <CloseButton onClick={onClose} class="ml-auto" />
      </div>
      <div class="min-h-0 flex-1 p-6">
        <InternalEditor
          {nested}
          {subtitle}
          {value}
          {editable}
          {saving}
          {onSave}
          {history}
          {footerExtra}
          {mode}
          {muse}
          {onClose}
          fill
        />
      </div>
    </div>
  {/if}
{:else}
  <Modal {open} {onClose} {title} width="max-w-6xl">
    <InternalEditor {subtitle} {value} {editable} {saving} {onSave} {history} {footerExtra} {mode} {muse} {onClose} />
  </Modal>
{/if}
