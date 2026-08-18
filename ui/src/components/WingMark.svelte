<script lang="ts">
  // The notched-triangle "A"-delta glyph in flat warm gold. Since the wordmark
  // was retired this carries the brand on its own.
  //
  // `size` is an inline style rather than a computed class because Tailwind
  // scans source text for class names: a `h-{size}` built at runtime is never
  // in the stylesheet, so the mark would render at its intrinsic size with no
  // error to explain it. Inline also beats any `class` a caller passes, so the
  // existing `class="h-5 w-5"` call sites keep working untouched.
  let { class: className, size }: { class?: string; size?: number } = $props()
</script>

<svg
  viewBox="0 0 20 20"
  class={className}
  style:width={size ? `${size}px` : undefined}
  style:height={size ? `${size}px` : undefined}
  aria-hidden="true"
>
  <path d="M10 2L18 18H12.6L10 12.4L7.4 18H2L10 2Z" fill="var(--theme-accent)" />
</svg>
