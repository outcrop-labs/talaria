<script lang="ts">
  import { cn } from '@/lib/cn'
  import { focusRing, type BaseProps } from './checkbox'

  // Switch-styled boolean — same API as Checkbox, heavier visual weight for
  // settings rows where the toggle IS the feature.
  let { checked, onChange, label, disabled, title, class: className }: BaseProps = $props()
</script>

<label {title} class={cn('flex cursor-pointer items-center gap-2 text-xs text-muted', disabled && 'cursor-default opacity-50', className)}>
  <span
    role="switch"
    aria-checked={checked}
    tabindex={disabled ? -1 : 0}
    onkeydown={(e) => {
      if (disabled) return
      if (e.key === ' ' || e.key === 'Enter') {
        e.preventDefault()
        onChange(!checked)
      }
    }}
    onclick={() => !disabled && onChange(!checked)}
    class={cn(
      // Spec §8 toggle: pill ~36×20; on = gold knob on a warm track;
      // off = muted knob on a raised track; disabled rows dim (label).
      'relative inline-block h-5 w-9 shrink-0 rounded-full border transition-colors',
      focusRing,
      checked ? 'border-[var(--theme-accent-border)] bg-accent-soft' : 'border-line bg-raised',
    )}
  >
    <span
      class={cn(
        'absolute top-1/2 h-3.5 w-3.5 -translate-y-1/2 rounded-full transition-all',
        checked ? 'left-[18px] bg-accent' : 'left-0.5 bg-muted',
      )}
    ></span>
  </span>
  {#if typeof label === 'string'}{label}{:else if label}{@render label()}{/if}
</label>
