// Live proof of the update engine's registry reads (cargo test -- --
// ignored). The wire protocol — anonymous token, manifest HEAD,
// Docker-Content-Digest, the index walk to the version label — is the
// part that can only be wrong against a real registry, and the registry
// that matters is the one CI actually publishes to. House rule:
// #[ignore]d, never CI.
//
//   cargo test --test update_live -- --ignored
//
// (No DATABASE_URL here: this suite proves the REGISTRY plane; the state
// row's Postgres behavior is covered by the pure tests in state.rs, which
// need no database.)

use talaria_api::update::registry::{
    fetch_version_label, is_digest, parse_image_ref, resolve_digest,
};

/// The trunk feed, resolved and walked — the exact read a scheduled check
/// makes. If CI published it (app-image.yml), this answers.
#[tokio::test]
#[ignore = "reads a real registry over the network"]
async fn the_trunk_feed_resolves_to_a_digest_and_version() {
    // The default ref, spelled out so a TALARIA_UPDATE_IMAGE override in
    // the local environment can't change what this test claims.
    let image = parse_image_ref("ghcr.io/outcrop-labs/talaria:main").expect("the trunk ref parses");

    // resolve_latest's pieces, in its order (the composite reads the env;
    // this test names its inputs).
    let digest = resolve_digest(&image)
        .await
        .expect("main resolves to a digest");
    assert!(is_digest(&digest), "a pin-shaped digest: {digest}");

    let version = fetch_version_label(&image, &digest)
        .await
        .expect("the trunk image carries its version label");
    assert!(
        version.starts_with("sha-") || !version.contains(' '),
        "a version a human reads: {version}"
    );

    // The same digest twice: registry reads are stable, and the pin a
    // check records is the pin the roll pulls.
    let again = resolve_digest(&image).await.expect("main resolves again");
    assert_eq!(again, digest, "the moving tag did not move mid-read");
}
