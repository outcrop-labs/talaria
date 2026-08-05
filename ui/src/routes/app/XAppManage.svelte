<script lang="ts">
  import { route } from '@/router'
  import AppSurface from '@/components/app/AppSurface.svelte'

  // An app's MANAGE surface (/x/<slug>/manage): grouped with core Manage views —
  // default-denied for members, grantable per person in Admin → People.
  const app = $derived(route.getParams('/x/:app/manage').app)
</script>

<AppSurface slug={app} surface="manage" />
