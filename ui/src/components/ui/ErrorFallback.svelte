<script lang="ts">
  import { cn } from '@/lib/cn'
  import { fade } from '@/lib/motion'
  import Button from './Button.svelte'
  import { isStaleChunkError, messageOf } from './error'

  // The error zero-state. Shaped like EmptyState so a broken view reads like
  // the rest of the app, not like a browser crash page. Every fallback names
  // WHAT broke and offers a way out (reload, retry). Never a stack trace: the
  // stack goes to the console, the person gets a sentence and a button.
  //
  // Where React needed the ErrorBoundary class, Svelte callers wrap in
  // <svelte:boundary> and render this in the `failed` snippet.
  let {
    error,
    reset,
    what,
    variant = 'full',
    class: className,
  }: {
    error: unknown
    /** Clears the error and re-renders. Omitted where retry can't help. */
    reset?: () => void
    /** What failed, in the user's words — "Boards", "this view". */
    what?: string
    variant?: 'full' | 'compact'
    class?: string
  } = $props()

  const stale = $derived(isStaleChunkError(error))
  const subject = $derived(what ?? 'This view')
  const title = $derived(stale ? 'Talaria updated while this tab was open' : `${subject} failed to load`)
  const hint = $derived(
    stale
      ? 'This tab is running an older build whose code is no longer on the server. Reload to pick up the new one.'
      : 'Something on this page failed while rendering. Reloading usually clears it; if it keeps happening, the detail below is what to report.',
  )
  const detail = $derived(stale ? '' : messageOf(error))
</script>

<!-- |global: this IS the component root, mounted by error boundaries when
     they trip — a local intro would never play (ANIMATIONS.md). -->
<div in:fade|global={{ duration: 150 }} class={cn(variant === 'full' ? 'grid h-full min-h-[60vh] place-items-center p-6' : 'px-2 py-6', 'text-center', className)}>
  <div class={cn('max-w-sm', variant === 'compact' && 'mx-auto')}>
    <!-- Not `mercury-text`: the brand gradient is for welcome moments. A
         failure gets the danger token, plainly. -->
    <div class={cn('mx-auto text-[color:var(--theme-danger)]', variant === 'full' ? 'mb-3 text-3xl' : 'mb-2 text-xl')}>⊘</div>
    <div class={cn('font-medium text-fg', variant === 'full' ? 'text-sm' : 'text-xs')}>{title}</div>
    <div class="mt-1 text-xs text-muted">{hint}</div>
    {#if detail}
      <div class="mt-3 max-h-24 overflow-y-auto rounded-xl border border-line-subtle px-3 py-2 text-left text-[11px] break-words text-muted">
        {detail}
      </div>
    {/if}
    <div class={cn('flex flex-wrap items-center justify-center gap-2', variant === 'full' ? 'mt-4' : 'mt-3')}>
      <Button size="sm" onclick={() => window.location.reload()}>Reload</Button>
      {#if reset && !stale}
        <Button size="sm" variant="outline" onclick={reset}>Try again</Button>
      {/if}
      {#if variant === 'full'}
        <a href="/" class="text-xs text-accent hover:underline">Back to chat</a>
      {/if}
    </div>
  </div>
</div>
