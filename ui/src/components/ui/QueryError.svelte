<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { errorMessage } from '@/lib/fetch-json'
  import type { QueryErrorProps } from './query-state'

  // The error branch on its own — for surfaces that fan several queries into one
  // view and need to render the failure at their own seam.
  let {
    error,
    title = 'Could not load this',
    onRetry,
    variant = 'full',
    class: className,
  }: QueryErrorProps = $props()

  const hint = $derived(errorMessage(error))
</script>

{#snippet icon()}
  <span class="text-[color:var(--theme-danger)]">⚠</span>
{/snippet}

{#snippet action()}
  <Button variant="outline" size="sm" onclick={onRetry}>
    Retry
  </Button>
{/snippet}

{#if variant === 'inline'}
  <div class={className}>
    <div class="text-xs text-[color:var(--theme-danger)]">{title}</div>
    <div class="mt-0.5 text-xs text-muted">{hint}</div>
    {#if onRetry}
      <Button variant="link" size="sm" class="mt-1" onclick={onRetry}>
        Retry
      </Button>
    {/if}
  </div>
{:else}
  <EmptyState {variant} class={className} {icon} {title} {hint} action={onRetry ? action : undefined} />
{/if}
