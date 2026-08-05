<script lang="ts">
  // No state — this block exists so the component is typed as TS. Without it
  // svelte-check treats the file as JS, and the eager import in router.ts
  // (a .ts module, allowJs off) reports an implicit-any module.
</script>

<div class="grid min-h-screen place-items-center p-6 text-center">
  <div>
    <div class="mercury-text mb-1 text-2xl font-semibold">404</div>
    <p class="text-sm text-muted">That view doesn’t exist (yet).</p>
    <a href="/" class="mt-3 inline-block text-sm text-accent hover:underline">
      Back to chat
    </a>
  </div>
</div>
