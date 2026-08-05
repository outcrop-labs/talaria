<script lang="ts" module>
  // A full, searchable emoji picker. The ~1900-emoji dataset (@emoji-mart/data) is
  // lazy-loaded on first open so it never weighs down the initial bundle. Renders
  // as a click-away popover (§7 popover shell); the caller positions the trigger.

  interface EmojiEntry {
    id: string
    native: string
    name: string
    keywords: string[]
  }

  let CACHE: EmojiEntry[] | null = null
  let CATEGORIES: Array<{ id: string; label: string; ids: string[] }> = []

  const CATEGORY_LABEL: Record<string, string> = {
    people: 'Smileys & People',
    nature: 'Animals & Nature',
    foods: 'Food & Drink',
    activity: 'Activity',
    places: 'Travel & Places',
    objects: 'Objects',
    symbols: 'Symbols',
    flags: 'Flags',
  }

  // A tiny fallback so the picker still works if the dataset import ever fails.
  const FALLBACK = '📄📝📌📚📁🗂️🧭🧠💡⚙️🚀🔧🔒🔑🎯✅📊📈🧪🗺️🏷️💬📣🌐🔍⭐🔥❤️⚠️🐛🤖👤🏢💰📅🎨'.split('')

  async function loadEmoji(): Promise<EmojiEntry[]> {
    if (CACHE) return CACHE
    try {
      const data = (await import('@emoji-mart/data')).default as {
        emojis: Record<string, { id: string; name: string; keywords: string[]; skins: Array<{ native: string }> }>
        categories: Array<{ id: string; emojis: string[] }>
      }
      const entries: EmojiEntry[] = []
      for (const key of Object.keys(data.emojis)) {
        const e = data.emojis[key]!
        const native = e.skins?.[0]?.native
        if (native) entries.push({ id: e.id, native, name: e.name, keywords: e.keywords ?? [] })
      }
      CATEGORIES = data.categories.map((c) => ({ id: c.id, label: CATEGORY_LABEL[c.id] ?? c.id, ids: c.emojis }))
      CACHE = entries
    } catch {
      CACHE = FALLBACK.map((native, i) => ({ id: `f${i}`, native, name: native, keywords: [] }))
      CATEGORIES = [{ id: 'common', label: 'Common', ids: CACHE.map((e) => e.id) }]
    }
    return CACHE
  }
</script>

<script lang="ts">
  import Input from '@/components/ui/Input.svelte'
  import { cn } from '@/lib/cn'
  import { scale, POP } from '@/lib/motion'
  import { popHeader, popPanel } from '@/components/chat/chat-chrome'

  let {
    onPick,
    onClear,
    onClose,
    align = 'left',
  }: {
    onPick: (emoji: string) => void
    onClear?: () => void
    onClose: () => void
    align?: 'left' | 'right'
  } = $props()

  let el = $state<HTMLDivElement | null>(null)
  let all = $state<EmojiEntry[]>(CACHE ?? [])
  let q = $state('')

  $effect(() => {
    if (!CACHE) void loadEmoji().then((v) => (all = v))
  })

  function onDocMousedown(e: MouseEvent) {
    if (el && !el.contains(e.target as Node)) onClose()
  }

  const byId = $derived(new Map(all.map((e) => [e.id, e])))
  const results = $derived.by(() => {
    const term = q.trim().toLowerCase()
    if (!term) return null // null → show categorized browse view
    return all
      .filter((e) => e.name.toLowerCase().includes(term) || e.id.includes(term) || e.keywords.some((k) => k.includes(term)))
      .slice(0, 96)
  })

  function categoryEntries(ids: string[]): EmojiEntry[] {
    return ids.map((id) => byId.get(id)).filter((e): e is EmojiEntry => !!e)
  }
</script>

{#snippet grid(entries: EmojiEntry[])}
  <div class="grid grid-cols-8 gap-0.5">
    {#each entries as e (e.id)}
      <button type="button" title={e.name} onclick={() => onPick(e.native)} class="grid h-8 place-items-center rounded-md text-lg transition-colors hover:bg-hover">
        {e.native}
      </button>
    {/each}
  </div>
{/snippet}

<svelte:document onmousedown={onDocMousedown} />

<div
  bind:this={el}
  in:scale={POP}
  class={cn(
    popPanel,
    'absolute top-full z-30 mt-1 w-72 p-2',
    align === 'right' ? 'right-0' : 'left-0',
  )}
>
  <Input autofocus size="sm" bind:value={q} placeholder="Search emoji" class="mb-2" />
  <div class="max-h-64 overflow-y-auto">
    {#if all.length === 0}
      <div class="py-8 text-center font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Loading</div>
    {:else if results}
      {#if results.length === 0}
        <div class="py-8 text-center text-xs text-muted">No emoji found.</div>
      {:else}
        {@render grid(results)}
      {/if}
    {:else}
      {#each CATEGORIES as cat (cat.id)}
        {@const entries = categoryEntries(cat.ids)}
        {#if entries.length > 0}
          <div class="mb-1">
            <div class={cn(popHeader, 'px-1 py-1')}>{cat.label}</div>
            {@render grid(entries)}
          </div>
        {/if}
      {/each}
    {/if}
  </div>
  {#if onClear}
    <button
      type="button"
      onclick={onClear}
      class="mt-1 w-full rounded-md py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg"
    >
      Remove icon
    </button>
  {/if}
</div>
