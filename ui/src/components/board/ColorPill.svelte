<script lang="ts">
  import { Palette } from '@lucide/svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import { cn } from '@/lib/cn'
  import { LABEL_CSS } from './field-pills'

  // Color picker as a dropdown swatch GRID — same pill grammar as every other
  // property. Standalone (value/onChange) so the detail rail and any future
  // surface can use it without a full PillCtx.
  let {
    value,
    onChange,
    disabled,
    class: className,
  }: {
    value: string | null
    onChange: (c: string | null) => void
    disabled?: boolean
    class?: string
  } = $props()

  const colors = Object.keys(LABEL_CSS) as Array<keyof typeof LABEL_CSS>
</script>

{#snippet palette()}
  <Palette size={11} />
{/snippet}

{#if disabled}
  {#if value}
    <span class={cn('inline-flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted', className)}>
      <span class="h-2.5 w-2.5 rounded-full" style:background={LABEL_CSS[value as keyof typeof LABEL_CSS]}></span>
      {value}
    </span>
  {/if}
{:else}
  <DropdownMenu align="left" class={className} items={[]}>
    {#snippet trigger(open)}
      <FieldPill
        active={open}
        empty={!value}
        dot={value ? LABEL_CSS[value as keyof typeof LABEL_CSS] : undefined}
        icon={value ? undefined : palette}
        title="Color-code this ticket"
      >
        {value ? '' : 'Color'}
      </FieldPill>
    {/snippet}
    {#snippet content(close)}
      <div class="p-1">
        <div class="grid grid-cols-4 gap-2 p-1">
          {#each colors as c (c)}
            <button
              title={c}
              onclick={() => {
                onChange(value === c ? null : c)
                close()
              }}
              class={cn(
                'grid h-6 w-6 place-items-center rounded-full transition-all',
                value === c ? 'ring-1 ring-[color:var(--theme-fg)]/60 ring-offset-2 ring-offset-[color:var(--theme-panel)]' : 'hover:ring-1 hover:ring-line',
              )}
              style:background={LABEL_CSS[c]}
            ></button>
          {/each}
        </div>
        {#if value}
          <button
            onclick={() => {
              onChange(null)
              close()
            }}
            class="mt-1 w-full rounded-md px-2 py-1 text-left text-xs text-muted transition-colors hover:bg-hover hover:text-danger"
          >
            Clear color
          </button>
        {/if}
      </div>
    {/snippet}
  </DropdownMenu>
{/if}
