<script lang="ts">
  import { hostOf } from '../lib/instances'
  import { open, shell } from '../lib/state.svelte'

  // The highlighted row is the instance the person was last in. (It used to
  // be the last one opened FROM HERE, so a switch made inside an instance
  // left the wrong row lit.) The × is always in the Tab order — visible on
  // hover and on keyboard focus — rather than display:none until hovered,
  // which kept remove out of keyboard reach.
</script>

<nav class="overflow-y-auto" aria-label="Instances">
  {#if shell.instances.length === 0}
    <p class="px-4 py-3 text-xs text-dim">No instances yet.</p>
  {:else}
    {#each shell.instances as instance (instance.id)}
      {@const host = hostOf(instance.url)}
      <div class="group relative">
        <button
          class="row w-full border-b border-hairline py-2.5 pr-9 pl-4 text-left {shell.recentId === instance.id
            ? 'bg-raised shadow-[inset_2px_0_0_var(--color-gold)]'
            : 'hover:bg-hover'}"
          title={instance.url}
          onclick={() => void open(instance.id)}
        >
          <div class="truncate text-[13px] leading-tight text-readout">{instance.label}</div>
          {#if host !== instance.label}
            <div class="truncate font-mono text-[10px] leading-tight text-muted">{host}</div>
          {/if}
        </button>
        <button
          class="absolute top-1/2 right-2 -translate-y-1/2 rounded px-1.5 py-0.5 text-xs text-muted opacity-0 group-focus-within:opacity-100 group-hover:opacity-100 hover:text-danger focus-visible:opacity-100"
          title="Remove instance"
          aria-label="Remove {instance.label}"
          onclick={() => (shell.confirmRemove = instance.id)}
        >
          ×
        </button>
      </div>
    {/each}
  {/if}
</nav>
