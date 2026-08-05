<script lang="ts">
  import { route } from '@/router'
  import AppSurface from '@/components/app/AppSurface.svelte'

  // An enabled app's WORK surface, mounted at /x/<slug> — full pane, native.
  const app = $derived(route.getParams('/x/:app').app)
</script>

<AppSurface slug={app} surface="work" />
