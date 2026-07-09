import { createFileRoute, redirect } from '@tanstack/react-router'

// Chat lives inside Comms now (Agents section of the unified sidebar).
export const Route = createFileRoute('/_app/chat')({
  beforeLoad: () => {
    throw redirect({ to: '/comms' })
  },
})
