- **Task workflows speak Rust**: `/api/workflows` and `/api/workflows/{id}` —
  the list (any member; workflows ground what agents will be told), create,
  the field-at-a-time patch, and delete. The match/skills/toolkits payload
  rides as jsonb the DB itself orders, so both runtimes read the same bytes
  back; and the group's quiet corners held: there is no 404 (a missed id
  still answers ok), and an empty patch runs no SQL at all — which is why it
  answers ok even for a garbage `{id}` while a one-field patch takes the
  recorded platform-500 divergence. The zod array surface behind the body
  (element checks before array length, trim before length, unknown keys
  stripped at every level) is probed against the ui's own zod and pinned in
  the api's tests.
