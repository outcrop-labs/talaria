<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { DiffLine } from './line-diff'

  let { diff, fallback }: { diff: DiffLine[] | null; fallback: string } = $props()

  const changed = $derived(diff?.some((l) => l.type === 'add' || l.type === 'del') ?? false)
</script>

{#if !diff}
  <div class="h-full overflow-y-auto rounded-lg border border-line p-3">
    <p class="mb-2 text-xs text-muted">Too large to diff. Showing the revision's full content.</p>
    <pre class="whitespace-pre-wrap font-mono text-xs text-fg">{fallback}</pre>
  </div>
{:else if !changed}
  <div class="grid h-full place-items-center rounded-lg border border-line text-sm text-muted">
    Identical to the editor's current content.
  </div>
{:else}
  <div class="h-full overflow-y-auto rounded-lg border border-line py-1">
    {#each diff as l, idx (idx)}
      {#if l.type === 'skip'}
        <div class="px-3 py-1 text-center text-[11px] text-muted">
          ··· {l.count} unchanged line{l.count === 1 ? '' : 's'} ···
        </div>
      {:else}
        <div
          class={cn(
            'whitespace-pre-wrap px-3 font-mono text-xs leading-5',
            l.type === 'add' && 'bg-[color-mix(in_srgb,var(--theme-success)_14%,transparent)] text-fg',
            l.type === 'del' && 'bg-[color-mix(in_srgb,var(--theme-danger)_12%,transparent)] text-muted',
            l.type === 'same' && 'text-muted',
          )}
        >
          <span class="mr-2 inline-block w-3 select-none text-muted">{l.type === 'add' ? '+' : l.type === 'del' ? '−' : ''}</span>{l.text || ' '}
        </div>
      {/if}
    {/each}
  </div>
{/if}
