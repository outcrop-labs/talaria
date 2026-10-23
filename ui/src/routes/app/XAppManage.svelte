<script lang="ts">
  import { route } from '@/router'
  import AppSurface from '@/components/app/AppSurface.svelte'

  // An app's MANAGE surface (/x/<slug>/manage, and every route below it):
  // grouped with core Manage views — default-denied for members, grantable
  // per person in Admin → People. The route gate holds the whole subtree to
  // the manage grant (appViewFor, lib/app-grants.ts).
  const app = $derived(route.params.app ?? '')
</script>

<AppSurface slug={app} surface="manage" />
