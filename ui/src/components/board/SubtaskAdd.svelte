<script lang="ts">
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
  <button
    type="button"
    onclick={() => (open = true)}
    class="w-full rounded-md px-1 py-0.5 text-left font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg"
  >
    + Add sub-task
  </button>
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
