<script lang="ts">
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import { fade, fly, QUICK } from '@/lib/motion'
  import { portal } from '@/lib/portal'
  import { dismissToast, toastList, type ToastItem } from '@/lib/toast.svelte'
  import { navigateHref } from '@/router'

  // The one toast host, mounted once in AppLayout. Portaled to <body> like
  // the modal layer so no ancestor stacking context can clip it.
  const items = $derived(toastList())

  const toneStatus: Record<ToastItem['tone'], DotStatus> = {
    info: 'accent',
    success: 'ok',
    danger: 'danger',
  }

  const open = (t: ToastItem) => {
    dismissToast(t.id)
    if (t.href) void navigateHref(t.href)
  }
</script>

<!-- z-[70]: above the modal layer (z-50) and popovers (z-[60]) — a toast is
     news, and news outranks whatever dialog it arrived during. The container
     itself is transparent to pointers; each card opts back in. -->
<div
  use:portal
  aria-live="polite"
  class="pointer-events-none fixed bottom-4 right-4 z-[70] flex w-[min(92vw,340px)] flex-col gap-2"
>
  {#each items as t (t.id)}
    <div
      in:fly={{ x: 16, duration: 220 }}
      out:fade={QUICK}
      role="status"
      class="pointer-events-auto relative rounded-[10px] border border-line bg-panel p-3 pr-9 shadow-[var(--theme-shadow-2)]"
    >
      <!-- The whole card is the click target: a toast's one job is "go
           there". The close affordance sits OUTSIDE that target so ending a
           toast never navigates. -->
      <button type="button" onclick={() => open(t)} class="flex w-full items-start gap-2.5 text-left">
        <StatusDot status={toneStatus[t.tone]} class="mt-[7px]" />
        <span class="min-w-0">
          <span class="block font-sans text-sm font-medium leading-5 text-fg">{t.title}</span>
          {#if t.body}<span class="mt-0.5 block truncate font-sans text-xs text-muted">{t.body}</span>{/if}
        </span>
      </button>
      <CloseButton onClick={() => dismissToast(t.id)} size={13} label="Dismiss" class="absolute right-1.5 top-1.5" />
    </div>
  {/each}
</div>
