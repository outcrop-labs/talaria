<script lang="ts">
  import type { Snippet } from 'svelte'

  let { children }: { children: Snippet } = $props()
</script>

<div class="px-2 py-1 text-[11px] leading-relaxed text-muted">{@render children()}</div>
