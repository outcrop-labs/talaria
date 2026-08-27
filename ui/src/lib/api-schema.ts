// Wire-level id/email schemas shared by the API route bodies.
//
// Why this file exists: the routes under src/routes/api re-declared
// z.string().uuid() 81 times across 53 files with no shared source of truth.
// Any change to the id rule — tightening the format, adding an error message,
// normalizing casing — would have to find every copy by hand, and one missed
// copy means the wire silently disagrees with itself. One declaration, one
// import, one place to change.
//
// Rule of thumb for what belongs here: shapes the WIRE agrees on — the id and
// email formats every path param and body field parses with. Not business
// objects (invite payloads, folder actions) — those stay in the route that
// owns them and compose these primitives.
import { z } from 'zod'

export const Uuid = z.string().uuid()

export const Email = z.string().email()

/** The bare `{ id }` body that revoke/delete endpoints repeat. */
export const IdBody = z.object({ id: Uuid })
