// Where the fleet lives on disk. The render engine that OWNS the fleet's
// filesystem is Rust's now (`api/src/fleet/`); this module is the smallest
// thing the two live TS readers depend on — the agent gateway reads
// fleet/fleet.json, the provider catalog reads the fleet's env for provider
// keys — so neither drags the (deleted) TS render web back in.
import { join, resolve } from 'node:path'

export const FLEET_DIR = () => process.env.TALARIA_FLEET_DIR ?? resolve(process.cwd(), '../fleet')

/** The fleet's env file (agent keys + compose interpolation) — Talaria-owned. */
export const FLEET_ENV = () => join(FLEET_DIR(), '.env')
