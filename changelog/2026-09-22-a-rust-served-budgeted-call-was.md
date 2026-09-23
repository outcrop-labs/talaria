- **A Rust-served budgeted call was flying blind**: the priced-view SQL that
  `spendSince` reads bound its cache multipliers as `$2`/`$3`, colliding with
  the caller's own second bind — the statement couldn't even prepare
  (`integer * text`), and the budget check's error-swallowing `.ok()?` turned
  that into "no spend data" on every Rust-served gateway call with a budget
  attached. The multipliers are values in the SQL text now, exactly as TS
  interpolates them; the statement prepares and returns real spend.
