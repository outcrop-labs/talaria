<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { Maximize2, ChevronLeft } from '@lucide/svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import type { RichEditorHandle } from '@/components/ui/rich-editor'
  import Markdown from '@/components/ui/Markdown.svelte'
  import type { Mentionable } from '@/components/chat/mentions.svelte'
  import { fly } from '@/lib/motion'
  import { cn } from '@/lib/cn'

  // Description with a Read (rendered markdown) / Edit (WYSIWYG) toggle plus an
  // expand button for comfortable full-screen reading. Keeps a local draft so the
  // read view reflects edits without refetching the ticket.
  let {
    title,
    value,
    canEdit,
    mentions,
    onSave,
    editor = $bindable(null),
  }: {
    title: string
    value: string
    canEdit: boolean
    mentions?: Mentionable[]
    onSave: (md: string) => void
    /** Exposes the live editor handle (selection-scoped Muse) — bind:editor.
     *  (React forwarded a ref; here it's a bindable prop fed by bind:this.) */
    editor?: RichEditorHandle | null
  } = $props()

  let draft = $state(value)
  let mode = $state<'read' | 'edit'>(canEdit && !value ? 'edit' : 'read')
  let reading = $state(false)
  // Bumped on every save so the other (unfocused) editor instance remounts with
  // the latest draft — keeps the inline + expanded views in sync.
  let rev = $state(0)

  const save = (md: string) => {
    draft = md
    rev += 1
    onSave(md)
  }

  const modes = ['read', 'edit'] as const
</script>

{#snippet modeToggle()}
  {#if canEdit}
    <div class="flex rounded-md border border-line p-0.5">
      {#each modes as m (m)}
        <button
          onclick={() => (mode = m)}
          class={cn(
            'rounded px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
            mode === m ? 'bg-raised text-fg' : 'text-muted hover:text-fg',
          )}
        >
          {m}
        </button>
      {/each}
    </div>
  {/if}
{/snippet}

{#snippet body(minHeight: string, readMax?: string)}
  {#if mode === 'edit' && canEdit}
    {#key rev}
      <RichEditor bind:this={editor} value={draft} editable {mentions} onSave={save} placeholder="Add detail" {minHeight} />
    {/key}
  {:else if draft}
    <div class={cn('rounded-lg border border-line bg-card px-4 py-3 font-sans text-sm leading-relaxed', readMax && `${readMax} overflow-y-auto`)}>
      <Markdown children={draft} />
    </div>
  {:else}
    <div class="rounded-lg border border-dashed border-line px-4 py-6 text-center font-sans text-xs text-muted">
      No description{canEdit ? '. Switch to Edit to add one.' : '.'}
    </div>
  {/if}
{/snippet}

<div>
  <div class="mb-2 flex items-center gap-2">
    <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Description</div>
    <div class="ml-auto flex items-center gap-1">
      {@render modeToggle()}
      <button
        onclick={() => (reading = true)}
        title="Expand"
        aria-label="Expand description"
        class="grid h-6 w-6 place-items-center rounded text-muted transition-colors hover:bg-hover hover:text-fg"
      >
        <Maximize2 size={13} />
      </button>
    </div>
  </div>

  {@render body('16rem')}

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
        <Button variant="ghost" size="xs" class="gap-1 py-1 hover:bg-hover" onclick={() => (reading = false)}>
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
            <div class="mx-auto max-w-2xl font-sans text-sm leading-relaxed">
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
