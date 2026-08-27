<script lang="ts" module>
  import Button from '@/components/ui/Button.svelte'
  // A full, searchable emoji picker. The ~1900-emoji dataset (@emoji-mart/data) is
  // lazy-loaded on first open so it never weighs down the initial bundle. Renders
  // in the §7 popover shell (ui/Popover.svelte); the caller supplies the trigger.

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
  import type { Snippet } from 'svelte'
  import Input from '@/components/ui/Input.svelte'
  import { cn } from '@/lib/cn'
  import { listStagger } from '@/lib/motion'
  import { popHeader } from '@/components/chat/chat-chrome'
  import Popover from '@/components/ui/Popover.svelte'

  let {
    trigger,
    onPick,
    onClear,
    align = 'left',
    up = false,
  }: {
    /** The icon button that opens the picker — Popover owns its toggle. */
    trigger: Snippet<[boolean]>
    onPick: (emoji: string) => void
    onClear?: () => void
    align?: 'left' | 'right'
    /** Open upward — triggers docked at the bottom of the viewport (the chat
     *  composer's smiley tile), where a downward panel would fly off-screen. */
    up?: boolean
  } = $props()

  let open = $state(false)
  let all = $state<EmojiEntry[]>(CACHE ?? [])
  let q = $state('')

  // The shell keeps this component mounted, so "first open" is a state change,
  // not a mount: the dataset import stays off the initial bundle.
  $effect(() => {
    if (open && !CACHE) void loadEmoji().then((v) => (all = v))
  })
  // The panel used to unmount on close, which reset the search for free.
  $effect(() => {
    if (!open) q = ''
  })

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

{#snippet grid(entries: EmojiEntry[], close: () => void)}
  <div class="grid grid-cols-8 gap-0.5" use:listStagger>
    {#each entries as e (e.id)}
      <button type="button" title={e.name} onclick={() => { close(); onPick(e.native) }} class="grid h-8 select-none place-items-center rounded-md text-lg transition-colors dither-fill">
        {e.native}
      </button>
    {/each}
  </div>
{/snippet}

<Popover bind:open {trigger} {align} {up} class="w-72 p-2">
  {#snippet content(close)}
    <Input autofocus size="sm" bind:value={q} placeholder="Search emoji" class="mb-2" />
    <div class="max-h-64 overflow-y-auto">
      {#if all.length === 0}
        <div class="py-8 text-center font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Loading</div>
      {:else if results}
        {#if results.length === 0}
          <div class="py-8 text-center text-xs text-muted">No emoji found.</div>
        {:else}
          {@render grid(results, close)}
        {/if}
      {:else}
        {#each CATEGORIES as cat (cat.id)}
          {@const entries = categoryEntries(cat.ids)}
          {#if entries.length > 0}
            <div class="mb-1">
              <div class={cn(popHeader, 'px-1 py-1')}>{cat.label}</div>
              {@render grid(entries, close)}
            </div>
          {/if}
        {/each}
      {/if}
    </div>
    {#if onClear}
      <Button variant="ghost" size="xs" class="mt-1 w-full py-1" onclick={() => { close(); onClear?.() }}>
        Remove icon
      </Button>
    {/if}
  {/snippet}
</Popover>
