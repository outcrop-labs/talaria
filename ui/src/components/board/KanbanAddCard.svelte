<script lang="ts">
  import Input from '@/components/ui/Input.svelte'

  let { onAdd }: { onAdd: (title: string) => void } = $props()

  let open = $state(false)
  let title = $state('')
  const submit = () => {
    const t = title.trim()
    if (!t) {
      open = false
      return
    }
    title = ''
    onAdd(t)
  }
</script>

{#if !open}
  <button
    type="button"
    onclick={() => (open = true)}
    class="w-full rounded-md px-2 py-1.5 text-left font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:bg-hover hover:text-fg"
  >
    + Add card
  </button>
{:else}
  <Input
    autofocus
    bind:value={title}
    onkeydown={(e) => (e.key === 'Enter' ? submit() : e.key === 'Escape' ? (open = false) : null)}
    onblur={submit}
    placeholder="Card title"
    size="sm"
    class="w-full"
  />
{/if}
