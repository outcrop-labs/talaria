<script lang="ts">
  import type { Snippet } from 'svelte'
  import { X } from '@lucide/svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { cn } from '@/lib/cn'

  /** The access sections' one row shape: name, tool scope, remove. */
  let {
    name,
    tools,
    dim,
    onRemove,
    removeTitle,
  }: {
    name: string
    tools: Snippet
    dim?: boolean
    onRemove: () => void
    removeTitle: string
  } = $props()
</script>

<div class="flex items-center gap-3 py-1.5">
  <span class={cn('w-44 shrink-0 truncate font-sans text-sm leading-5', dim ? 'text-muted' : 'text-fg')} title={name}>{name}</span>
  <span class="min-w-0 flex-1">{@render tools()}</span>
  <IconButton size="sm" title={removeTitle} onclick={onRemove}><X size={16} /></IconButton>
</div>
