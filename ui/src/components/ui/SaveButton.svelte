<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import type { ButtonVariant } from './button'
  import type { ControlSize } from './control'
  import Button from './Button.svelte'
  import { cn } from '@/lib/cn'
  import { fade, QUICK } from '@/lib/motion'
  import { useSavedFlash } from './save-button.svelte'

  interface Props extends Omit<HTMLButtonAttributes, 'onclick'> {
    variant?: ButtonVariant
    size?: ControlSize
    onSave: () => Promise<unknown> | unknown
  }

  // Runs `onSave`, disables while pending, flashes "Saved".
  let { onSave, children, class: className, disabled, ...rest }: Props = $props()

  let busy = $state(false)
  const flash = useSavedFlash()
</script>

<span class={cn('inline-flex items-center gap-2', className)}>
  <Button
    {...rest}
    disabled={disabled || busy}
    onclick={() => {
      busy = true
      void Promise.resolve(onSave())
        .then(() => flash.flash())
        .finally(() => (busy = false))
    }}
  >
    {#if children}{@render children()}{:else}Save{/if}
  </Button>
  <!-- Confirmation reads as mono telemetry in the success signal (spec §8). -->
  {#if flash.saved}
    <span in:fade={{ duration: 150 }} out:fade={QUICK} class="font-mono text-[10px] uppercase tracking-[0.05em] text-success">Saved</span>
  {/if}
</span>
