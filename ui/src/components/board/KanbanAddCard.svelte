<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
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
  <Button variant="ghost" size="xs" class="w-full py-1.5 text-left hover:dither-fill" onclick={() => (open = true)}>
    + Add card
  </Button>
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
