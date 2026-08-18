<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import SaveDialog from './SaveDialog.svelte'

  // An agent-produced image in chat, with a hover affordance to keep it: "Save
  // to Artifacts" copies the file out of the agent's container into a durable
  // FILE artifact, optionally straight into a folder (science / memes / etc).
  let { src, alt }: { src: string; alt: string } = $props()

  let open = $state(false)
</script>

<span class="group relative my-2 inline-block">
  <img {src} {alt} class="max-h-96 rounded-lg border border-line" />
  <!-- Secondary action (§8): raised tile + hairline + mono readout label. -->
  <Button variant="outline" size="xs" class="absolute right-2 top-2 py-1 text-muted opacity-0 transition-opacity hover:text-fg group-hover:opacity-100" onclick={() => (open = true)}>
    Save to Artifacts
  </Button>
  {#if open}<SaveDialog {src} onClose={() => (open = false)} />{/if}
</span>
