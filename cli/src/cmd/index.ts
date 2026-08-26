// The command tree — one declaration per command, the single source help is
// rendered from. Commands register themselves here as they are ported; each
// port deletes its bash script in the same commit (docs/CHANGELOG follow).

import type { Group } from '../cli'
import { boxCommand } from './box'
import { deployCommand } from './deploy'
import { devCommand } from './dev'
import { resetCommand } from './reset'
import { setupCommand } from './setup'
import { worktreeCommand } from './worktree'

export const tree: Group = {
  kind: 'group',
  name: 'talaria',
  summary: 'Talaria — every way to drive the repo, from one place',
  children: [setupCommand, devCommand, worktreeCommand, resetCommand, boxCommand, deployCommand],
}
