- **The profile speaks Rust**: `/api/me` — the three preference reads
  (preferred model, platform-default reasoning effort, IANA zone) and the
  profile PUT: display name (users row AND the live session, via the
  KEEPTTL patch — the SPA's corner never waits for a re-login), the member
  model gate (enforced at the route, not just hidden in the picker), and the
  zone check. The zone validator is a probed contract, not a regex: TS asks
  Intl to resolve the name, and node's and bun's ICUs were probed on a
  100+-spelling table where they agreed on every row — IANA names match
  CASE-INSENSITIVELY ("utc", "Etc/gmt+5" resolve), offset forms `±HH`,
  `±HH:MM`, `±HHMM` resolve up to hour 23 / minute 59, and `Factory` is
  refused. The port mirrors that grammar (chrono-tz's tzdb + a hand-rolled
  offset parser) and pins the same table in its tests. The route migrates
  by EXACT pathname — the me.* siblings (mcp, assistant, events) are their
  own planes that move whole with their batches. Byte-diffed against TS on
  the same DB: the full zod 400 table, the trim corners (a spaces-only name
  is legal to zod and stores ""; a padded zone trims before it validates),
  the application order (a name in one PUT lands before a refused
  model/zone answers the same PUT — TS applies fields in sequence and the
  port invents no transaction), the member gate's exact 403 sentence with no
  write behind it, and the 405s.
