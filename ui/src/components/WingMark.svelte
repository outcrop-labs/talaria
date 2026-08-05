<script lang="ts">
  // The notched-triangle "A"-delta glyph in flat warm gold.
  let { class: className }: { class?: string } = $props()
</script>

<svg viewBox="0 0 20 20" class={className} aria-hidden="true">
  <path d="M10 2L18 18H12.6L10 12.4L7.4 18H2L10 2Z" fill="var(--theme-accent)" />
</svg>
