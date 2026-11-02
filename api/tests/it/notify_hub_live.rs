// Live-DB proof for the hub, replay and the inbox API (docs/notifications/live-and-inbox.md).
// A placeholder the contract PR registers, so W2-H fills this module
// without touching main.rs. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it notify_hub_live:: -- --ignored

#[tokio::test]
#[ignore = "placeholder: W2-H fills this module"]
async fn placeholder() {}
