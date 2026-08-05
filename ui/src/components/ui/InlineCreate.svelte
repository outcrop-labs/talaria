<script lang="ts">
  import { Plus, CornerDownLeft } from '@lucide/svelte'
  import { scale } from '@/lib/motion'
  import Button from './Button.svelte'
  import Input from './Input.svelte'

  // A primary "+ <label>" button that expands into a full-width input. Enter (or
  // blur) submits, Escape cancels. Reuse anywhere you want an unobtrusive create
  // affordance that only takes space while in use.
  let {
    label,
    placeholder,
    onSubmit,
    class: className,
    size = 'sm',
  }: {
    label: string
    placeholder?: string
    onSubmit: (value: string) => void
    class?: string
    size?: 'sm' | 'md'
  } = $props()

  let open = $state(false)
  let value = $state('')
  let cancel = false

  const submit = () => {
    open = false
    const v = value.trim()
    const cancelled = cancel
    cancel = false
    value = ''
    if (v && !cancelled) onSubmit(v)
  }
</script>

{#if !open}
  <div class={className}>
    <Button {size} onclick={() => (open = true)}>
      <Plus size={size === 'sm' ? 15 : 17} />
      {label}
    </Button>
  </div>
{:else}
  <!-- The React version grew from the left (scaleX 0.55 → 1); svelte's scale is
       uniform, so this is a whole-control pop from the same origin — close enough. -->
  <div class={className} in:scale={{ duration: 150, start: 0.55 }} style:transform-origin="left">
    <div class="relative">
      <Input
        autofocus
        bind:value
        placeholder={placeholder ?? label}
        onkeydown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur()
          else if (e.key === 'Escape') {
            cancel = true
            e.currentTarget.blur()
          }
        }}
        onblur={submit}
        {size}
        class={size === 'sm' ? 'pr-8' : 'pr-9'}
      />
      <!-- Enter hint — pressing Enter (or clicking) submits. mousedown fires
          before the input's blur so the value is still there. -->
      <button
        type="button"
        tabindex={-1}
        aria-label="Submit"
        onmousedown={(e) => {
          e.preventDefault()
          ;(e.currentTarget.previousElementSibling as HTMLInputElement | null)?.blur()
        }}
        class="absolute inset-y-0 right-2 grid place-items-center text-muted transition-colors hover:text-accent"
      >
        <CornerDownLeft size={size === 'sm' ? 14 : 16} />
      </button>
    </div>
  </div>
{/if}
