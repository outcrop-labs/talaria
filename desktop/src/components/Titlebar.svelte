<script lang="ts">
  import { desktopWindow, type TitlebarMode } from '../lib/instances'

  // Themed window chrome for the launcher webview. Same contract as the
  // instance UI's DesktopTitlebar: drag the bar, buttons do min/max/close,
  // traffic-light side follows the OS. Renders nothing unless mode is themed.
  let { mode }: { mode: TitlebarMode } = $props()
  const mac = /Mac|iPhone|iPad/.test(navigator.userAgent)

  const drag = (e: MouseEvent) => {
    if (e.button !== 0) return
    if ((e.target as HTMLElement | null)?.closest('button')) return
    void desktopWindow('startDragging')
  }
</script>

{#if mode === 'themed'}
  <div
    class="flex h-9 shrink-0 items-center border-b border-hairline bg-panel select-none"
    onmousedown={drag}
    ondblclick={() => void desktopWindow('toggleMaximize')}
    role="presentation"
  >
    {#if mac}
      <div class="flex items-center px-1">{@render controls()}</div>
      <div class="min-w-0 flex-1"></div>
    {:else}
      <div class="min-w-0 flex-1"></div>
      <div class="flex items-center px-1">{@render controls()}</div>
    {/if}
  </div>
{/if}

{#snippet btn(label: string, action: 'minimize' | 'toggleMaximize' | 'close', glyph: string)}
  <button
    type="button"
    title={label}
    aria-label={label}
    class="grid h-7 w-9 place-items-center rounded-sm text-muted hover:bg-raised hover:text-readout {action === 'close'
      ? 'hover:bg-danger/15 hover:text-danger'
      : ''}"
    onclick={() => void desktopWindow(action)}
  >
    {glyph}
  </button>
{/snippet}

{#snippet controls()}
  {#if mac}
    {@render btn('Close', 'close', '×')}
    {@render btn('Minimize', 'minimize', '–')}
    {@render btn('Maximize', 'toggleMaximize', '□')}
  {:else}
    {@render btn('Minimize', 'minimize', '–')}
    {@render btn('Maximize', 'toggleMaximize', '□')}
    {@render btn('Close', 'close', '×')}
  {/if}
{/snippet}
