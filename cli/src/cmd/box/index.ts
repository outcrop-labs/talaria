// `talaria box` — the devbox command group. Each box is a containerized dev
// environment per task: a full repo clone on its own branch, own sidecars
// seeded from the primary stack, own private fleet. docs/DEVBOX.md.

import type { Group } from '../../cli'
import { newCommand } from './new'
import { lsCommand } from './ls'
import { enterCommand } from './enter'
import { installCommand } from './install'
import { buildCommand, rmCommand, startCommand, stopCommand } from './lifecycle'
import { seedCommand } from './seed'

export const boxCommand: Group = {
  kind: 'group',
  name: 'box',
  summary: 'devboxes — a containerized dev environment per task (docs/DEVBOX.md)',
  children: [newCommand, lsCommand, enterCommand, installCommand, seedCommand, stopCommand, startCommand, rmCommand, buildCommand],
}
