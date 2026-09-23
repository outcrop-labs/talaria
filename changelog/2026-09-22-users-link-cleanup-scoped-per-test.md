- **The users-link live tests scope their cleanup to the test's own email.**
  The suite's six tests each began with a domain-wide delete of every
  `@link-test.invalid` user — and libtest runs a binary's tests concurrently,
  so one test's cleanup could drop a sibling's seeded admin row mid-flight:
  the re-sign-in test then linked onto nothing and created a fresh member,
  failing "the link must never touch the role" on CI's slower runners while
  passing on a fast box. Each test now deletes only its own email, the same
  isolation rule the suite's siblings (attribution's subdomains,
  workchains' tags) already followed. With fitness_arming's key split (its
  own entry), the live suite's remaining binaries were audited for the same
  disease: push_live serializes on its keypair mutex by design and every
  other binary partitions rows per test — this was the last unguarded one.
  Verified: the binary green five consecutive runs against scratch
  postgres+redis (both fixed binaries green together); `cargo fmt --check`
  clean; every other live binary's cleanup audited for shared-row deletes.
