- **A re-drafted email converges on the approval already waiting, instead of
  queueing a twin.** A retried agent run re-drafts the same outbound email —
  reworded body, recipients reformatted — and every draft used to become its
  own p0 SEND EMAIL card: the outcrop brief held six cards for one reply to
  Cooper Beverage, and approving two of them was a double-send to a client.
  `queue_action` now matches a new gmail_send against still-pending rows by
  principal + recipient addresses + subject with the formatting boiled off,
  and returns the existing row with "an identical draft is already waiting"
  — nothing new queued, nothing announced twice. A decided row never
  matches: after a reject, a fresh draft is a fresh ask. Verified live: the
  re-draft from the incident report (display names vs bare addresses, flipped
  case, different body) collapses onto one pending row
  (`api/tests/pending_dedupe_live.rs`, house-ignored), and on outcrop the
  brief's approval dismissal lands and survives the sweep under #361's
  binary — the "can't dismiss" half of the report was the pre-#361 binary
  that ran until 15:14 UTC today.
