import { createFileRoute, redirect } from '@tanstack/react-router'

// The fleet overview folded into Agents (its health is on Home + the Agents
// header). Keep the path working for old links.
export const Route = createFileRoute('/_app/fleet')({
  beforeLoad: () => {
    throw redirect({ to: '/agents' })
  },
})
