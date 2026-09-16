<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { fade } from '@/lib/motion'

  // An app throw stops HERE. The rest of Talaria keeps running; this pane
  // shows the crash so an in-platform builder can fix it. Host ErrorFallback
  // never shows a stack; app crashes do — that's the point of the surface.
  let {
    error,
    reset,
    name,
    compiling = false,
  }: {
    error: unknown
    reset?: () => void
    name: string
    compiling?: boolean
  } = $props()

  const crash = $derived.by(() => {
    if (error instanceof Error) return { message: error.message || error.name, stack: error.stack ?? '' }
    return { message: String(error ?? 'unknown error'), stack: '' }
  })
</script>

<div in:fade|global={{ duration: 150 }} class="p-8">
  <div class="mx-auto max-w-[var(--page-width)] rounded-lg border border-danger/40 bg-panel p-6">
    <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">{compiling ? 'Build failed' : 'App crashed'}</div>
    <div class="mt-1 font-sans text-sm font-medium text-fg">
      {name} hit an error and was stopped so the rest of Talaria could keep running.
    </div>
    <pre
      class="mt-4 max-h-72 overflow-auto rounded-md border border-line bg-surface p-3 text-left font-mono text-[11px] break-words whitespace-pre-wrap text-muted"
    >{crash.message}{crash.stack && crash.stack !== crash.message ? `\n${crash.stack}` : ''}</pre>
    <div class="mt-4 flex flex-wrap gap-2">
      {#if reset}
        <Button size="sm" onclick={reset}>Try again</Button>
      {/if}
    </div>
  </div>
</div>
