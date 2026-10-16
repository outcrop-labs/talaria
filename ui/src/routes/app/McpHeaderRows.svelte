<script lang="ts">
  import { Plus, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { emptyHeaderRow, type HeaderRow } from './mcp'

  /** Free-form auth headers — name and value, as many as the server wants.
   *  The one shape every credential form shares (custom server, install,
   *  the card's credentials editor), so the password field is spelled once:
   *  type=password, autocomplete off, never echoed back. */
  let {
    rows = $bindable(),
    namePlaceholder = 'Authorization',
    valuePlaceholder = 'Bearer …',
    addLabel = 'Another header',
  }: {
    rows: HeaderRow[]
    namePlaceholder?: string
    valuePlaceholder?: string
    addLabel?: string
  } = $props()
</script>

<div class="space-y-1.5">
  {#each rows as r, i (i)}
    <div class="grid grid-cols-[10rem_1fr_auto] items-center gap-2">
      <Input size="sm" bind:value={r.name} placeholder={namePlaceholder} aria-label="Header name" class="font-[var(--font-mono)]" autocomplete="off" />
      <Input size="sm" bind:value={r.value} type="password" placeholder={valuePlaceholder} aria-label="Header value" autocomplete="off" />
      <button
        type="button"
        class="grid h-7 w-7 place-items-center rounded-md text-muted transition-colors hover:text-danger"
        title="Remove this header"
        onclick={() => (rows = rows.filter((_, j) => j !== i))}
      >
        <X size={13} />
      </button>
    </div>
  {/each}
  <Button size="xs" variant="ghost" class="gap-1 text-muted" onclick={() => (rows = [...rows, emptyHeaderRow()])}>
    <Plus size={12} /> {addLabel}
  </Button>
</div>
