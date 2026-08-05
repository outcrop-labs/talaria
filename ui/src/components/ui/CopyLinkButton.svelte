<script lang="ts">
  import CopyButton from './CopyButton.svelte'

  // Copies an absolute link to `path` — thin wrapper kept for existing call
  // sites; new code can use CopyButton directly.
  let {
    path,
    label,
    class: className,
    title = 'Copy link',
  }: {
    path: string
    label?: string
    class?: string
    title?: string
  } = $props()
</script>

<CopyButton {path} {label} class={className} {title} />
