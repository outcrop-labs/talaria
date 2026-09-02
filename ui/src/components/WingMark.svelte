<script lang="ts">
  // The three-stroke wing logomark — Talaria's mark on the marketing site,
  // now the platform's too. The paths are verbatim from that favicon
  // (talaria-website/static/favicon.svg); only the wrapper `<g>` differs.
  // The favicon parks the raw ~1100-unit art at translate(22.1 12)
  // scale(0.03334) — a small wing (about 18×37 of the 64 box) centred with
  // room for the dark ground around it. A bare mark has no ground, so this
  // transform maps the same art to ~90% of the viewBox instead, centred on
  // (32,32); the geometry itself is untouched.
  //
  // THE GROUND IS NOT HERE ON PURPOSE. The rounded near-black square is the
  // favicon's business (it is exactly what /favicon.svg ships); in the nav
  // rail and on the login panel the wing sits directly on the surface.
  //
  // Fill stays var(--theme-accent) rather than the favicon's fixed #c8b46c
  // so the one component reads correctly in dark and light themes both.
  //
  // `size` is an inline style rather than a computed class because Tailwind
  // scans source text for class names: a `h-{size}` built at runtime is never
  // in the stylesheet, so the mark would render at its intrinsic size with no
  // error to explain it. Inline also beats any `class` a caller passes, so the
  // existing `class="h-5 w-5"` call sites keep working untouched.
  let { class: className, size }: { class?: string; size?: number } = $props()
</script>

<svg
  viewBox="0 0 64 64"
  class={className}
  style:width={size ? `${size}px` : undefined}
  style:height={size ? `${size}px` : undefined}
  aria-hidden="true"
>
  <g fill="var(--theme-accent)" transform="translate(16.377 0.54) scale(0.05242)">
    <path d="M52.67,647.53c-5.86,3.38-12.06,3.95-18.11.36-3.85-2.28-8.65-7.79-8.64-14.32l.14-128.28c.01-13.64,9.11-22.49,19.86-28.64l115.22-65.97c8.65-4.95,16.84-12.44,16.9-23.25l.63-110.63c.08-13.64,9.09-22.53,20.08-28.72L469.57,95.56l74.35-41.53c5.87-3.28,13.78-1.29,18.72,1.97,4.95,3.26,7.61,9.25,7.6,16.33l-.13,127.25c-.01,11.27-5.59,20.81-15.18,26.29l-316.9,180.95c-8.87,5.07-17.74,12.91-17.8,23.73l-.56,104.15c-.07,13.39-8.74,21.41-19.49,27.62l-147.5,85.21Z" />
    <path d="M52.02,895.25c-6.24,3.69-12.93,2.44-17.78-.26-5.31-2.96-8.44-8.65-8.43-15.61l.15-133.02c.01-11.29,6.24-20.66,15.53-26.14l214.33-126.4c8.84-5.21,18.25-12.02,18.3-23.33l.51-100.49c.08-15.06,9.55-23.39,21.54-30.32l247.17-142.78c5.92-3.42,12.25-2.94,18.02.39,4.58,2.64,8.78,8.2,8.78,15.1l-.11,127.24c0,8.1-3.75,18.72-11.25,23.05l-223.02,129.06c-9.52,5.51-18.57,13.19-18.62,25.13l-.45,105.09c-.06,13.76-8.73,21.64-19.62,28.1l-245.06,145.17Z" />
    <path d="M54.37,1144.48c-6.76,4.1-12.31,5.08-18.8,1.8-4.95-2.5-9.68-8.15-9.68-15.45v-136.32c.01-14.36,8.93-22.3,19.96-28.93l306.56-184.35c8.86-5.33,17.61-12.82,17.67-23.81l.59-104c.08-13.74,8.76-22.48,19.72-28.81l154.01-88.97c5.51-3.18,12.82-1.46,17.42,1.21,4.96,2.88,8.3,8.74,8.3,15.44v128.53c0,8.1-3.32,19.23-10.71,23.52l-131.93,76.78c-7.9,4.6-13.74,13.98-13.77,22.77l-.36,106.41c-.05,13.85-8.5,21.93-19.38,28.46l-81.72,49.04-257.87,156.64Z" />
  </g>
</svg>
