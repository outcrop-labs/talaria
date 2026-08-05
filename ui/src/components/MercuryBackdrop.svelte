<script lang="ts">
  // Mercury is matte: the backdrop is the flat ground surface — no grids,
  // no glows, no drifting blooms. Kept as a component so existing layout
  // imports (the _app layout, login, join) stay stable.
  // (The script block exists so TS importers see a typed module.)
</script>
<div aria-hidden="true" class="pointer-events-none fixed inset-0 -z-10 bg-surface"></div>
