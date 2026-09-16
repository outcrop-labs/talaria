<script lang="ts">
  import { Minus, Square, X } from '@lucide/svelte'
  import { inDesktopShell } from '@/lib/desktop-shell'
  import {
    desktopSettings,
    desktopWindow,
    loadDesktopSettings,
  } from '@/lib/desktop-settings.svelte'
  import { cn } from '@/lib/cn'

  // Themed window chrome for the Talaria desktop shell. Renders nothing in a
  // browser, nothing when the operator picked the OS titlebar or none. The
  // drag surface is the bar itself; buttons are excluded so a click on close
  // is not a drag. Traffic-light side follows the OS: left on macOS, right
  // everywhere else.
  const settings = $derived(desktopSettings())
  const mac = $derived(/Mac|iPhone|iPad/.test(navigator.userAgent))

  $effect(() => {
    void loadDesktopSettings()
  })

  const show = $derived(inDesktopShell() && settings.titlebar === 'themed')

  const drag = (e: MouseEvent) => {
    if (e.button !== 0) return
    if ((e.target as HTMLElement | null)?.closest('button')) return
    desktopWindow('startDragging')
  }
</script>

{#if show}
  <div
    class="flex h-9 shrink-0 items-center border-b border-line bg-sidebar select-none"
    onmousedown={drag}
    ondblclick={() => desktopWindow('toggleMaximize')}
    role="presentation"
  >
    {#if mac}
      <div class="flex items-center gap-0.5 px-2">
        {@render controls()}
      </div>
      <div class="min-w-0 flex-1"></div>
    {:else}
      <div class="min-w-0 flex-1"></div>
      <div class="flex items-center gap-0.5 px-1">
        {@render controls()}
      </div>
    {/if}
  </div>
{/if}

{#snippet controls()}
  {#if !mac}
    <button
      type="button"
      title="Minimize"
      aria-label="Minimize"
      class={cn(
        'grid h-7 w-9 place-items-center rounded-sm text-muted transition-colors hover:bg-raised hover:text-fg',
      )}
      onclick={() => desktopWindow('minimize')}
    >
      <Minus size={12} strokeWidth={1.5} />
    </button>
    <button
      type="button"
      title="Maximize"
      aria-label="Maximize"
      class="grid h-7 w-9 place-items-center rounded-sm text-muted transition-colors hover:bg-raised hover:text-fg"
      onclick={() => desktopWindow('toggleMaximize')}
    >
      <Square size={11} strokeWidth={1.5} />
    </button>
  {/if}
  <button
    type="button"
    title="Close"
    aria-label="Close"
    class="grid h-7 w-9 place-items-center rounded-sm text-muted transition-colors hover:bg-danger/15 hover:text-danger"
    onclick={() => desktopWindow('close')}
  >
    <X size={13} strokeWidth={1.5} />
  </button>
  {#if mac}
    <button
      type="button"
      title="Minimize"
      aria-label="Minimize"
      class="grid h-7 w-9 place-items-center rounded-sm text-muted transition-colors hover:bg-raised hover:text-fg"
      onclick={() => desktopWindow('minimize')}
    >
      <Minus size={12} strokeWidth={1.5} />
    </button>
    <button
      type="button"
      title="Maximize"
      aria-label="Maximize"
      class="grid h-7 w-9 place-items-center rounded-sm text-muted transition-colors hover:bg-raised hover:text-fg"
      onclick={() => desktopWindow('toggleMaximize')}
    >
      <Square size={11} strokeWidth={1.5} />
    </button>
  {/if}
{/snippet}
