<script lang="ts">
  import { cn } from '@/lib/cn'
  import { focusRing, type BaseProps } from './checkbox'

  let { checked, onChange, label, disabled, title, bare, class: className }: BaseProps = $props()
</script>

{#if bare}
  <!-- Bare cell checkbox: no label element — the row around it is the hit
       target and the name comes from title/aria. See checkbox.ts BaseProps. -->
  <input
    type="checkbox"
    {checked}
    {disabled}
    {title}
    aria-label={title}
    onchange={(e) => onChange(e.currentTarget.checked)}
    class={cn('accent-accent', focusRing, className)}
  />
{:else}
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
{/if}
