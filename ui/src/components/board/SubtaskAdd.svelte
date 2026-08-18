<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'

  /** Inline add for sub-tasks — quiet trigger, Enter creates, Esc closes. */
  let { onAdd }: { onAdd: (title: string) => Promise<void> | void } = $props()

  let open = $state(false)
  let title = $state('')
  const submit = () => {
    const v = title.trim()
    if (!v) {
      open = false
      return
    }
    title = ''
    open = false
    void onAdd(v)
  }
</script>

{#if !open}
  <Button variant="ghost" size="xs" class="w-full px-1 py-0.5 text-left" onclick={() => (open = true)}>
    + Add sub-task
  </Button>
{:else}
  <Input
    autofocus
    bind:value={title}
    onkeydown={(e) => (e.key === 'Enter' ? submit() : e.key === 'Escape' ? (open = false) : null)}
    onblur={submit}
    placeholder="Sub-task title"
    size="sm"
    class="w-full"
  />
{/if}
