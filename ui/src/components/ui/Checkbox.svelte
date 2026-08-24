<script lang="ts">
  import { cn } from '@/lib/cn'
  import { focusRing, type BaseProps } from './checkbox'

  let { checked, onChange, label, disabled, title, class: className }: BaseProps = $props()
</script>

<label {title} class={cn('flex cursor-pointer select-none items-center gap-1.5 text-xs text-muted', disabled && 'cursor-default opacity-50', className)}>
  <input
    type="checkbox"
    {checked}
    {disabled}
    onchange={(e) => onChange(e.currentTarget.checked)}
    class={cn('accent-accent', focusRing)}
  />
  {#if typeof label === 'string'}{label}{:else if label}{@render label()}{/if}
</label>
