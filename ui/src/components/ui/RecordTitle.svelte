<script lang="ts">
  import type { Snippet } from 'svelte'
  import EmojiPicker from './EmojiPicker.svelte'
  import Input from './Input.svelte'
  import { cn } from '@/lib/cn'
  import { inlineEditKeys } from './control'

  // The one record-title cluster: emoji trigger + title, read or editable —
  // the copy the artifact editor, the KB doc editor and the KB space editor
  // each hand-rolled. It renders two siblings, not a wrapper, so the owning
  // toolbar's own gap/alignment keeps applying between them.
  //
  // Writes nothing itself: the surface keeps its save path and its dirty
  // rules, and decides on commit whether there is anything to save.
  let {
    icon,
    onIconPick,
    onIconClear,
    iconFallback = '📄',
    value,
    onInput,
    onCommit,
    onCancel,
    editing,
    placeholder = 'Untitled',
    class: className,
    meta,
  }: {
    /** The record's icon; `null` renders `iconFallback` in the trigger. */
    icon: string | null
    onIconPick: (icon: string) => void
    onIconClear: () => void
    /** Shown when no icon is set (📄 doc/artifact, 📚 space). */
    iconFallback?: string
    /** The title/name as the surface holds it. */
    value: string
    /** Every keystroke — the surface owns the state and its dirty flag. */
    onInput: (value: string) => void
    /** Blur or Enter. The surface decides if there is anything to save. */
    onCommit: () => void
    /** Escape — restore the saved value, so the later blur saves nothing. */
    onCancel: () => void
    /** Edit mode swaps the heading for the input; read mode shows the heading. */
    editing: boolean
    placeholder?: string
    /** Merged over the input — spaces run one step up (`text-xl`). */
    class?: string
    /** Read-mode sub-line under the heading (timestamp, breadcrumb note). */
    meta?: Snippet
  } = $props()
</script>

<div class="shrink-0">
  <EmojiPicker onPick={onIconPick} onClear={onIconClear}>
    {#snippet trigger()}
      <button type="button" class="rounded-md px-1 text-xl leading-none transition-colors dither-fill" title="Set icon">
        {icon ?? iconFallback}
      </button>
    {/snippet}
  </EmojiPicker>
</div>
{#if editing}
  <Input
    {value}
    {placeholder}
    oninput={(e) => onInput(e.currentTarget.value)}
    onblur={onCommit}
    onkeydown={inlineEditKeys(onCancel)}
    class={cn('min-w-0 flex-1 border-0 bg-transparent text-lg font-semibold focus:border-0', className)}
  />
{:else}
  <div class="min-w-0 flex-1">
    <h1 class="truncate font-sans text-lg font-semibold text-fg">{value}</h1>
    {#if meta}{@render meta()}{/if}
  </div>
{/if}