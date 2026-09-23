<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import { Maximize2, ChevronLeft } from '@lucide/svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import type { RichEditorHandle } from '@/components/ui/rich-editor'
  import Markdown from '@/components/ui/Markdown.svelte'
  import type { Mentionable } from '@/components/chat/mentions.svelte'
  import { fly } from '@/lib/motion'

  // Description with a Read (rendered markdown) / Edit (WYSIWYG) toggle plus an
  // expand button for comfortable full-screen reading. Keeps a local draft so the
  // read view reflects edits without refetching the ticket.
  let {
    title,
    value,
    canEdit,
    mentions,
    onSave,
    mode = $bindable('read'),
    editor = $bindable(null),
  }: {
    title: string
    value: string
    canEdit: boolean
    mentions?: Mentionable[]
    onSave: (md: string) => void
    /** Read/Edit is the PARENT'S to know: the ticket gates its Muse bar on
     *  "the description is being edited". The parent owns the initial too
     *  (it used to live here as "canEdit && empty → edit"), so bind it —
     *  the fallback only serves a caller that doesn't care. */
    mode?: 'read' | 'edit'
    /** Exposes the live editor handle (selection-scoped Muse) — bind:editor.
     *  (React forwarded a ref; here it's a bindable prop fed by bind:this.) */
    editor?: RichEditorHandle | null
  } = $props()

  // svelte-ignore state_referenced_locally -- reason: seed-once draft; the rev counter remounts the other instance with fresh content instead of re-seeding
  let draft = $state(value)
  let reading = $state(false)
  // Bumped on every save so the other (unfocused) editor instance remounts with
  // the latest draft — keeps the inline + expanded views in sync.
  let rev = $state(0)

  const save = (md: string) => {
    draft = md
    rev += 1
    onSave(md)
  }

  // The read/edit switch is the kit's xs Segmented — the same control the
  // artifact and KB doc editors use for this identical choice.
  const MODE_OPTIONS = [
    { id: 'read', label: 'Read' },
    { id: 'edit', label: 'Edit' },
  ] as const
</script>

{#snippet modeToggle()}
  {#if canEdit}
    <Segmented size="xs" options={MODE_OPTIONS} value={mode} onChange={(m) => (mode = m)} />
  {/if}
{/snippet}

<!-- The inline description is a fixed anchor: one height whether the text is
     a line or a page, scrolling inside the box. Long reading uses Expand.
     The height is a viewport clamp, not a share of whatever sits below, so
     tab content cannot resize it. The expand sheet is a sibling of the
     scroller (not a child) so this box's overflow does not clip it — it
     still covers the modal, whose panel is the positioned ancestor. -->
<div class="flex h-[clamp(9rem,24vh,14rem)] shrink-0 flex-col">
  <div class="mb-2 flex shrink-0 items-center gap-2">
    <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Description</div>
    <div class="ml-auto flex items-center gap-1">
      {@render modeToggle()}
      <IconButton
        size="sm"
        class="h-6 w-6 rounded"
        title="Expand"
        aria-label="Expand description"
        onclick={() => (reading = true)}
      >
        <Maximize2 size={13} />
      </IconButton>
    </div>
  </div>

  <div class="min-h-0 flex-1 overflow-hidden">
    {#if mode === 'edit' && canEdit}
      {#key rev}
        <RichEditor bind:this={editor} class="h-full" value={draft} editable fill {mentions} onSave={save} placeholder="Add detail" />
      {/key}
    {:else if draft}
      <div class="h-full overflow-y-auto rounded-lg border border-line bg-card px-4 py-3 font-sans text-sm leading-relaxed">
        <Markdown children={draft} />
      </div>
    {:else}
      <div class="grid h-full place-items-center rounded-lg border border-dashed border-line px-4 text-center font-sans text-xs text-muted">
        No description{canEdit ? '. Switch to Edit to add one.' : '.'}
      </div>
    {/if}
  </div>

  <!-- Expanded view — slides in over the whole ticket modal (no stacked modal).
       The modal panel is `relative`, so inset-0 covers it edge to edge.
       (framer's ease [0.4,0,0.2,1] → the fly wrapper's default cubicOut.)
       Deviation from the panel grammar's 8–16px travel: this is a full-cover
       sheet, so it keeps its ported full-width slide; duration clamped from
       220ms to the contract's 200ms ceiling. -->
  {#if reading}
    <div
      class="absolute inset-0 z-30 flex flex-col overflow-hidden rounded-xl border border-line bg-panel shadow-[var(--theme-shadow-3)]"
      transition:fly={{ x: '100%', duration: 200 }}
    >
      <div class="flex items-center gap-3 border-b border-line-subtle px-5 py-3">
        <Button variant="ghost" size="xs" class="gap-1 py-1 dither-fill" onclick={() => (reading = false)}>
          <ChevronLeft size={14} /> Back
        </Button>
        <div class="min-w-0 flex-1 truncate text-center font-sans text-sm font-semibold text-fg">{title}</div>
        <div class="flex w-[4.5rem] justify-end">
          {@render modeToggle()}
        </div>
      </div>
      {#if mode === 'edit' && canEdit}
        <div class="min-h-0 flex-1">
          {#key rev}
            <RichEditor value={draft} editable bare fill {mentions} onSave={save} placeholder="Add detail" />
          {/key}
        </div>
      {:else}
        <div class="min-h-0 flex-1 overflow-y-auto px-6 py-5">
          {#if draft}
            <div class="mx-auto max-w-[var(--read-width)] font-sans text-sm leading-relaxed">
              <Markdown children={draft} />
            </div>
          {:else}
            <div class="font-sans text-sm text-muted">No description yet.</div>
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>
