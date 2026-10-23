<script lang="ts">
  import Button from '@/components/ui/Button.svelte'

  // An agent-produced image in chat, with a hover affordance to keep it: "Save
  // to Artifacts" copies the file out of the agent's container into a durable
  // FILE artifact, optionally straight into a folder (science / memes / etc).
  let { src, alt }: { src: string; alt: string } = $props()

  let open = $state(false)
  // Loaded on first open, not with this component. Markdown renders this, so
  // it is in @talaria/sdk's graph, and SaveDialog imports the router, which
  // cannot load outside a browser: a static import made every app's dev
  // build probe fail with "location is not defined".
  const saveDialog = () => import('./SaveDialog.svelte')
</script>

<span class="group relative my-2 inline-block">
  <img {src} {alt} class="max-h-96 rounded-lg border border-line" />
  <!-- Secondary action (§8): raised tile + hairline + mono readout label. -->
  <Button variant="outline" size="xs" class="absolute right-2 top-2 py-1 text-muted opacity-0 transition-opacity hover:text-fg group-hover:opacity-100" onclick={() => (open = true)}>
    Save to Artifacts
  </Button>
  {#if open}{#await saveDialog() then { default: SaveDialog }}<SaveDialog {src} onClose={() => (open = false)} />{/await}{/if}
</span>
