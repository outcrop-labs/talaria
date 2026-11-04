<script lang="ts">
  import { route } from '@/router'
  import AppSurface from '@/components/app/AppSurface.svelte'

  // An enabled app's WORK surface, mounted at /x/<slug> and at every route
  // below it (/x/<slug>/items/42) — full pane, native. One mount for all of
  // them: moving between the app's own routes keeps the surface and hands it
  // the new rest (AppSurface → useAppRoute). The slug comes off the matched
  // params, not getParams('/x/:app'), which compares the path case-sensitively
  // and throws where the router (case-insensitive) matched /X/<slug>.
  const app = $derived(route.params.app ?? '')
</script>

<AppSurface slug={app} surface="work" />
