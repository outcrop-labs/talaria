<script lang="ts">
  import { Minus, Square, X } from '@lucide/svelte'
  import { loadDesktopSettings, desktopWindow } from '@/lib/desktop-settings.svelte'
  import { controlsSide, themedTitlebar, type ControlsSide } from '@/lib/desktop-titlebar'
  import { cn } from '@/lib/cn'

  // Placed at both ends of a bar; only the end that matches the OS renders.
  let { at }: { at: ControlsSide } = $props()

  $effect(() => {
    void loadDesktopSettings()
  })

  const show = $derived(themedTitlebar() && controlsSide() === at)

  const minimize = { action: 'minimize', label: 'Minimize', icon: Minus, size: 12, danger: false } as const
  const maximize = { action: 'toggleMaximize', label: 'Maximize', icon: Square, size: 11, danger: false } as const
  const close = { action: 'close', label: 'Close', icon: X, size: 13, danger: true } as const
  const buttons = $derived(at === 'left' ? [close, minimize, maximize] : [minimize, maximize, close])
</script>

{#if show}
  <div class="flex shrink-0 items-center gap-0.5">
    {#each buttons as b (b.action)}
      <button
        type="button"
        title={b.label}
        aria-label={b.label}
        class={cn(
          'grid h-8 w-9 place-items-center rounded-md text-muted transition-colors duration-[120ms]',
          b.danger ? 'hover:bg-danger/15 hover:text-danger' : 'hover:bg-raised hover:text-fg',
        )}
        onclick={() => desktopWindow(b.action)}
      >
        <b.icon size={b.size} strokeWidth={1.5} />
      </button>
    {/each}
  </div>
{/if}
