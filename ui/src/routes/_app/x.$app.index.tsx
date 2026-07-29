import { createFileRoute } from '@tanstack/react-router'
import { AppSurface } from '@/components/app/app-surface'

// An enabled app's WORK surface, mounted at /x/<slug> — full pane, native.
export const Route = createFileRoute('/_app/x/$app/')({
  component: () => <AppSurface slug={Route.useParams().app} surface="work" />,
})
