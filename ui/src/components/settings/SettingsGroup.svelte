<script lang="ts">
  import type { Snippet } from 'svelte'
  import type { HTMLAttributes } from 'svelte/elements'
  import Panel from '@/components/ui/Panel.svelte'
  import { cn } from '@/lib/cn'

  // The settings card: rows in one panel, divided by hairlines. Children are
  // `SettingsRow`s or any row-shaped content. Never nest one in another.
  let { class: className, children, ...rest }: HTMLAttributes<HTMLElement> & { class?: string; children: Snippet } = $props()
</script>

<Panel class={cn('overflow-hidden p-0 [&>*+*]:border-t [&>*+*]:border-line-subtle', className)} {...rest}>
  {@render children()}
</Panel>
