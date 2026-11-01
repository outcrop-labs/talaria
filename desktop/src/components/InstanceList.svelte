<script lang="ts">
  import { hostOf } from '../lib/instances'
  import { open, shell, toggleMuted } from '../lib/state.svelte'

  // The highlighted row is the instance the person was last in. (It used to
  // be the last one opened FROM HERE, so a switch made inside an instance
  // left the wrong row lit.) The row's buttons — mute and × — are always in
  // the Tab order, visible on hover and on keyboard focus, rather than
  // display:none until hovered, which kept them out of keyboard reach; a
  // muted row keeps its bell showing, so the state is never hidden.
  //
  // The count is the instance's unread For you, the same share the dock
  // badge sums. Muting stops an instance's banners on this desktop; its
  // count still shows, here and on the badge.
</script>

<nav class="overflow-y-auto" aria-label="Instances">
  {#if shell.instances.length === 0}
    <p class="px-4 py-3 text-xs text-dim">No instances yet.</p>
  {:else}
    {#each shell.instances as instance (instance.id)}
      {@const host = hostOf(instance.url)}
      {@const muted = shell.muted.includes(instance.id)}
      {@const unread = shell.unread[instance.id] ?? 0}
      <div class="group relative">
        <button
          class="row w-full border-b border-hairline py-2.5 pr-16 pl-4 text-left {shell.recentId === instance.id
            ? 'bg-raised shadow-[inset_2px_0_0_var(--color-gold)]'
            : 'hover:bg-hover'}"
          title={instance.url}
          onclick={() => void open(instance.id)}
        >
          <div class="flex items-center gap-2">
            <span class="truncate text-[13px] leading-tight text-readout">{instance.label}</span>
            {#if unread > 0}
              <span
                class="shrink-0 rounded-full bg-raised px-1.5 font-mono text-[10px] leading-4 text-gold"
                aria-label="{unread} unread"
              >
                {unread > 99 ? '99+' : unread}
              </span>
            {/if}
          </div>
          {#if host !== instance.label}
            <div class="truncate font-mono text-[10px] leading-tight text-muted">{host}</div>
          {/if}
        </button>
        <button
          class="absolute top-1/2 right-8 -translate-y-1/2 rounded px-1.5 py-0.5 text-xs text-muted hover:text-readout focus-visible:opacity-100 {muted
            ? 'opacity-100'
            : 'opacity-0 group-focus-within:opacity-100 group-hover:opacity-100'}"
          title={muted
            ? 'Muted: no banners from this instance here. Click to unmute'
            : 'Mute banners from this instance on this desktop'}
          aria-label="{muted ? 'Unmute' : 'Mute'} {instance.label}"
          aria-pressed={muted}
          onclick={() => void toggleMuted(instance.id)}
        >
          <svg
            viewBox="0 0 16 16"
            class="h-3.5 w-3.5"
            fill="none"
            stroke="currentColor"
            stroke-width="1.4"
            aria-hidden="true"
          >
            <path d="M4 11V7.5a4 4 0 0 1 8 0V11l1 1.5H3L4 11Z" stroke-linejoin="round" />
            <path d="M6.5 14a1.6 1.6 0 0 0 3 0" stroke-linecap="round" />
            {#if muted}
              <path d="M2.5 2.5l11 11" stroke-linecap="round" />
            {/if}
          </svg>
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
