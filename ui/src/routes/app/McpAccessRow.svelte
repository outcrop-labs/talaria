<script lang="ts">
  import type { Snippet } from 'svelte'
  import { Trash2 } from '@lucide/svelte'
  import { cn } from '@/lib/cn'

  /** The access sections' one row shape: name · tool scope · quiet remove. */
  let {
    name,
    tools,
    dim,
    onRemove,
    removeTitle,
  }: {
    name: string
    tools?: Snippet
    dim?: boolean
    onRemove: () => void
    removeTitle: string
  } = $props()
</script>

<div data-dither-fill class="group flex items-center gap-3 rounded-md px-1.5 py-1 transition-colors">
  <span class="w-44 shrink-0 truncate">
    <span class={cn('font-sans text-sm', dim ? 'text-muted' : 'text-fg')}>{name}</span>
  </span>
  <span class="min-w-0 flex-1">
    {#if tools}{@render tools()}{:else}<span class="font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">all tools</span>{/if}
  </span>
  <!-- Fixed-width control cluster so every row's tool box ends flush. -->
  <span class="flex w-8 shrink-0 items-center justify-end">
    <button
      type="button"
      onclick={onRemove}
      title={removeTitle}
      class="text-muted opacity-0 transition-opacity hover:text-danger group-hover:opacity-100"
    >
      <Trash2 size={13} />
    </button>
  </span>
</div>
