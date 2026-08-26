#!/usr/bin/env bun
// The `talaria` command. Run from the repo root (`bun talaria …`) or directly
// (`bun cli/bin/talaria.ts …`). Everything interesting lives in src/.

import { realCtx } from '../src/ctx'
import { dispatch } from '../src/cli'
import { tree } from '../src/cmd'

// CliError never reaches here — dispatch prints it (through the color gate)
// and returns 1. This catch is the last-resort reporter for real bugs.
process.exit(await dispatch(realCtx(), tree, process.argv.slice(2)))
