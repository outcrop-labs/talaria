import { createFileRoute } from '@tanstack/react-router'
import { AppSurface } from '@/components/app/app-surface'

// An app's MANAGE surface (/x/<slug>/manage): grouped with core Manage views —
// default-denied for members, grantable per person in Admin → People.
export const Route = createFileRoute('/_app/x/$app/manage')({
  component: () => <AppSurface slug={Route.useParams().app} surface="manage" />,
})
