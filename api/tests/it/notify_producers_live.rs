// Live-DB proof for coalescing producers and channel levels (docs/notifications/producers.md).
// A placeholder the contract PR registers, so W2-P fills this module
// without touching main.rs. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it notify_producers_live:: -- --ignored

#[tokio::test]
#[ignore = "placeholder: W2-P fills this module"]
async fn placeholder() {}
