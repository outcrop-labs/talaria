<script lang="ts">
  import { open, remove, shell } from '../lib/state.svelte'

  let confirming = $state<string | null>(null)

  const hostOf = (url: string) => url.replace(/^https?:\/\//, '')
  const labelOf = (id: string) => shell.instances.find((i) => i.id === id)?.label ?? 'instance'
</script>

<nav class="flex-1 overflow-y-auto" aria-label="Instances">
  {#if shell.instances.length === 0}
    <p class="px-4 py-3 text-xs text-dim">No instances yet.</p>
  {:else}
    {#each shell.instances as instance (instance.id)}
      <div class="group relative">
        <button
          class="row w-full border-b border-hairline px-4 py-2.5 text-left {
            shell.activeId === instance.id
              ? 'bg-raised shadow-[inset_2px_0_0_var(--color-gold)]'
              : 'hover:bg-hover'
          }"
          title={instance.url}
          onclick={() => void open(instance.id)}
        >
          <div class="truncate text-[13px] leading-tight text-readout">{instance.label}</div>
          <div class="truncate font-mono text-[10px] leading-tight text-muted">
            {hostOf(instance.url)}
          </div>
        </button>
        <button
          class="absolute top-1/2 right-2 hidden -translate-y-1/2 rounded px-1 py-0.5 text-xs text-muted hover:text-danger group-hover:block"
          title="Remove instance"
          aria-label="Remove {instance.label}"
          onclick={() => (confirming = instance.id)}
        >
          ×
        </button>
      </div>
    {/each}
  {/if}
</nav>

{#if confirming}
  <div class="fixed inset-0 grid place-items-center bg-black/60 p-4" role="dialog" aria-modal="true">
    <div class="w-64 rounded-md border border-hairline bg-panel p-4">
      <p class="text-sm text-readout">Remove {labelOf(confirming)}?</p>
      <p class="mt-1 text-xs text-muted">
        Its saved session is deleted with it. The instance itself is untouched.
      </p>
      <div class="mt-4 flex justify-end gap-2">
        <button
          class="rounded px-3 py-1.5 text-xs text-muted hover:bg-hover hover:text-readout"
          onclick={() => (confirming = null)}
        >
          Cancel
        </button>
        <button
          class="rounded border border-danger px-3 py-1.5 font-mono text-xs text-danger hover:bg-danger/10"
          onclick={() => {
            if (!confirming) return
            const id = confirming
            confirming = null
            void remove(id)
          }}
        >
          REMOVE
        </button>
      </div>
    </div>
  </div>
{/if}
