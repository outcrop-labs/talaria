<script lang="ts">
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { filtersActive, kindOptions, type CenterFilters } from '@/lib/notify-center'
  import { CATEGORY_LABEL, CATEGORY_ORDER } from '@/lib/notify-matrix'
  import type { Category, ClassInfo } from '@/lib/notify-wire'

  // The center's filters: unread only, the categories as chips, and one
  // kind ("what happened") drawn from the registry's reasons. Each change is
  // a URL change (the page owns the URL): a filtered view is a link.
  let {
    filters,
    classes,
    onChange,
  }: {
    filters: CenterFilters
    classes: readonly ClassInfo[] | undefined
    onChange: (next: CenterFilters) => void
  } = $props()

  // Only the categories this person has classes in (a member has no
  // Workspace), in the matrix's order.
  const categories = $derived(CATEGORY_ORDER.filter((c) => (classes ?? []).some((k) => k.category === c)))
  const kinds = $derived(kindOptions(classes, filters.categories))

  const toggleCategory = (c: Category) => {
    const has = filters.categories.includes(c)
    const next = has ? filters.categories.filter((x) => x !== c) : [...filters.categories, c]
    // A kind outside the new categories would filter to nothing, silently.
    const keepKind = filters.kind !== null && kindOptions(classes, next).some((k) => k.id === filters.kind)
    onChange({ ...filters, categories: next, kind: keepKind ? filters.kind : null })
  }

  let kindValue = $state('')
  $effect(() => {
    kindValue = filters.kind ?? ''
  })
</script>

<div class="flex flex-wrap items-center gap-x-3 gap-y-2" data-notify-filters>
  <Checkbox checked={filters.unread} label="Unread only" onChange={(on) => onChange({ ...filters, unread: on })} />
  <div class="flex flex-wrap items-center gap-1.5">
    {#each categories as c (c)}
      <Chip onSelect={() => toggleCategory(c)} selected={filters.categories.includes(c)} title="Show only {CATEGORY_LABEL[c]}">
        {CATEGORY_LABEL[c]}
      </Chip>
    {/each}
  </div>
  {#if kinds.length}
    <Select
      size="sm"
      class="w-auto max-w-56"
      aria-label="What happened"
      bind:value={kindValue}
      onchange={() => onChange({ ...filters, kind: kindValue || null })}
    >
      <option value="">Anything</option>
      {#each kinds as k (k.id)}
        <option value={k.id}>{k.label}</option>
      {/each}
    </Select>
  {/if}
  {#if filtersActive(filters)}
    <button
      type="button"
      class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg"
      onclick={() => onChange({ unread: false, categories: [], kind: null })}
    >
      Clear
    </button>
  {/if}
</div>
