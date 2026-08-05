<script lang="ts">
  import { Search } from '@lucide/svelte'
  import { cn } from '@/lib/cn'

  // The §7 popover search row: hairline field on top, mono placeholder, ⌘K
  // hint. The hint is functional, not decorative (spec §7: no fabricated
  // affordances) — while the popover is open, ⌘K refocuses the field.
  let {
    value,
    onChange,
    placeholder,
    class: className,
  }: {
    value: string
    onChange: (v: string) => void
    placeholder: string
    class?: string
  } = $props()

  let inputRef = $state<HTMLInputElement | null>(null)

  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        inputRef?.focus()
        inputRef?.select()
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  })
</script>

<div class={cn('mb-1 flex items-center gap-1.5 rounded-md border border-line px-2', className)}>
  <Search size={12} class="shrink-0 text-muted" />
  <!-- svelte-ignore a11y_autofocus -->
  <input
    bind:this={inputRef}
    autofocus
    {value}
    oninput={(e) => onChange(e.currentTarget.value)}
    {placeholder}
    class="h-7 w-full min-w-0 bg-transparent font-mono text-[11px] tracking-[0.05em] text-fg outline-none placeholder:text-muted"
  />
  <span aria-hidden="true" class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-ink-dim">
    ⌘K
  </span>
</div>
