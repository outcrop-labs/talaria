<script lang="ts">
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
  <button
    type="button"
    onclick={() => (open = true)}
    class="absolute right-2 top-2 rounded-md border border-line bg-raised px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted opacity-0 transition-opacity hover:text-fg group-hover:opacity-100"
  >
    Save to Artifacts
  </button>
  {#if open}<SaveDialog {src} onClose={() => (open = false)} />{/if}
</span>
