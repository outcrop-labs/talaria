- **Object storage — built-in bucket, bring-your-own, or both.** Upload blobs
  can now live in a real S3-compatible bucket instead of local disk, three
  ways: the **built-in bucket** — a bundled MinIO container (dev-compose
  `minio` service, creds via `TALARIA_S3_*` env, bucket auto-created) so you
  get durable object storage with no cloud account; any **external**
  S3-compatible service (AWS S3, Backblaze B2, Cloudflare R2, MinIO) via
  endpoint/bucket/keys in Admin → Storage (secret sealed by secretbox); and an
  optional **replica** that mirrors every new upload to a second provider as
  it lands (fire-and-forget — a replica outage never blocks an upload), with a
  "Sync all" that backfills everything already stored and automatic read
  fallback to the mirror when the primary can't serve a blob. The client is a
  hand-rolled SigV4 signer over fetch — no SDK. Each upload's row records
  where ITS bytes live (`s3+internal://` / `s3://` / filesystem path), so
  switching modes never strands a file. Connection tests do a real
  write/read/delete round-trip; a background migration moves local files into
  the active bucket. Verified live 22/22 across two runs: external-bucket flow
  against a throwaway MinIO, then built-in mode with auto-created bucket,
  replica mirror-on-upload, full sync, and replica fallback after deleting the
  primary object.
