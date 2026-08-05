<script lang="ts">
  import type { HTMLButtonAttributes } from 'svelte/elements'
  import { buttonClasses, type ButtonVariant } from './button'
  import type { ControlSize } from './control'

  interface Props extends HTMLButtonAttributes {
    variant?: ButtonVariant
    size?: ControlSize
    /** bind:ref for imperative focus/measure at call sites that need it. */
    ref?: HTMLButtonElement | null
  }

  let {
    variant = 'primary',
    size = 'md',
    type = 'button',
    class: className,
    children,
    ref = $bindable(null),
    ...rest
  }: Props = $props()
</script>

<!-- The one button. Reuse everywhere — do not re-style buttons inline. -->
<button bind:this={ref} {type} class={buttonClasses({ variant, size, className: className as string | null })} {...rest}>
  {@render children?.()}
</button>
