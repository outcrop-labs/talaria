<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { popPanel, popRow } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { fade, scale, POP, QUICK } from '@/lib/motion'

  // Add a model to a provider: browse/search its live catalog (in the provider's
  // own order — OpenRouter lists newest first), or type any id (multi-model
  // providers serve more than the catalog reports). NOT a label — a model.
  let { catalog, existing, onAdd }: { catalog: string[]; existing: string[]; onAdd: (id: string) => void } = $props()

  let q = $state('')
  let open = $state(false)
  const suggestions = $derived(catalog.filter((m) => !existing.includes(m) && m.toLowerCase().includes(q.trim().toLowerCase())))
  const add = (id: string) => {
    onAdd(id)
    q = ''
  }
</script>

<div class="relative">
  <div class="flex items-center gap-2">
    <Input
      size="sm"
      bind:value={q}
      onfocus={() => (open = true)}
      onblur={() => (open = false)}
      onkeydown={(e) => e.key === 'Enter' && q.trim() && add(q.trim())}
      placeholder={`Add a model: browse the live catalog${catalog.length ? ` (${catalog.length})` : ''} or type an id`}
      class="flex-1"
    />
    <Button size="sm" onclick={() => q.trim() && add(q.trim())} disabled={!q.trim()}>
      Add
    </Button>
  </div>
  {#if open && suggestions.length > 0}
    <div
      in:scale={{ ...POP, start: 0.97 }}
      out:fade={QUICK}
      class={cn(popPanel, 'absolute z-10 mt-1 max-h-52 w-full origin-top overflow-y-auto')}
    >
      {#each suggestions as m (m)}
        <button
          type="button"
          onmousedown={(e) => {
            e.preventDefault()
            add(m)
          }}
          class={cn(popRow, 'font-mono text-xs text-muted hover:text-fg')}
        >
          {m}
        </button>
      {/each}
    </div>
  {/if}
</div>
