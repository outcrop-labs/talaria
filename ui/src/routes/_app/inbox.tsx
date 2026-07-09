import { createFileRoute, redirect } from '@tanstack/react-router'

// The inbox IS the home surface now (notifications + the day's dashboard).
export const Route = createFileRoute('/_app/inbox')({
  beforeLoad: () => {
    throw redirect({ to: '/' })
  },
})
