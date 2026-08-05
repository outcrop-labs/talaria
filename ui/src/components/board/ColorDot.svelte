<script lang="ts">
  // A plain colored dot for menu-item icons. Menu entry arrays are built in
  // plain TS (see MenuIcon in context-menu.svelte.ts — snippet or
  // [Component, props] tuple), so the raw <span> the React code inlined as an
  // icon becomes this tiny component. Callers pass the full class string
  // (size + rounded-full) so the rendered markup stays byte-identical.
  let { color, class: className }: { color?: string; class?: string } = $props()
</script>

<span class={className} style:background={color}></span>
