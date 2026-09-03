// Install mode — CAN this install roll itself, and if not, why not. The
// first check in every verb (the header of mod.rs): an install that isn't
// `image` mode gets a sentence for the panel, never a container.
//
// The order is the trust order. The kill switch outranks everything because
// it is the operator speaking; the install-mode signal comes next because
// it is the IMAGE speaking about itself (its Dockerfile sets
// TALARIA_INSTALL=image — no checkout install can ever carry it); dev
// detection last because it is the weakest inference (an env the dev
// server never stamps).
//
// The resolution itself is a pure function of the three values — edition
// 2024 makes env mutation unsafe in tests, and more to the point each arm
// is pinnable without touching the process env at all.

/// What this process is running from, and therefore whether the update
/// engine may act. The TS updater's `server`/`dev`/`off` taxonomy retires
/// with it; this is the Rust engine's, with `image` the one mode that rolls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallMode {
    /// A published image (TALARIA_INSTALL=image). The updater's domain:
    /// rolls are digest pulls of a new image and a slot cutover.
    Image,
    /// A git checkout under `bun server-entry.js`. The git-updater's
    /// domain; the container engine has nothing to say here.
    Checkout,
    /// Vite dev, or anything else unstamped. Reloads on file change;
    /// pulling an update under it would be chaos for no gain.
    Dev,
    /// TALARIA_UPDATER=off — the kill switch deployments that supervise
    /// the process themselves have always had.
    Off,
}

/// Resolve the mode from the environment (re-resolved by every caller —
/// routes, job, verbs — because it is one syscall and the honest shape).
pub fn install_mode() -> InstallMode {
    let read = |k: &str| std::env::var(k).ok();
    mode_from(
        read("TALARIA_UPDATER").as_deref(),
        read("TALARIA_INSTALL").as_deref(),
        read("TALARIA_RUNTIME").as_deref(),
    )
}

/// The resolution, pure: the three signals' values, in the header's trust
/// order. None means the env is unset.
pub fn mode_from(
    updater: Option<&str>,
    install: Option<&str>,
    runtime: Option<&str>,
) -> InstallMode {
    // The kill switch, verbatim from the TS updater: deployments that own
    // restarts their own way keep the env they already document.
    if updater == Some("off") {
        return InstallMode::Off;
    }
    // The image's own statement about itself (its Dockerfile ENV; a
    // checkout install never sets it).
    if install == Some("image") {
        return InstallMode::Image;
    }
    // server-entry.js stamps prod-server before importing the app graph;
    // vite dev never does. The one honest signal for "a server install".
    if runtime != Some("prod-server") {
        return InstallMode::Dev;
    }
    InstallMode::Checkout
}

impl InstallMode {
    /// The sentence the panel shows instead of buttons. Only `image` has
    /// nothing to refuse.
    pub fn refusal(&self) -> Option<&'static str> {
        match self {
            InstallMode::Image => None,
            InstallMode::Checkout => Some(
                "This install runs from a git checkout; the container updater is for image installs.",
            ),
            InstallMode::Dev => {
                Some("Dev picks up code changes on its own; updates are for image installs.")
            }
            InstallMode::Off => {
                Some("Updates are switched off on this install (TALARIA_UPDATER=off).")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kill_switch_outranks_the_image_signal() {
        assert_eq!(
            mode_from(Some("off"), Some("image"), None),
            InstallMode::Off
        );
        assert_eq!(
            InstallMode::Off.refusal(),
            Some("Updates are switched off on this install (TALARIA_UPDATER=off).")
        );
    }

    #[test]
    fn image_mode_is_the_images_own_statement() {
        // Runtime is irrelevant once the image has spoken — an image
        // install under a dev-ish runtime is still an image install.
        assert_eq!(mode_from(None, Some("image"), None), InstallMode::Image);
        assert_eq!(
            mode_from(None, Some("image"), Some("prod-server")),
            InstallMode::Image
        );
        assert_eq!(InstallMode::Image.refusal(), None);
    }

    #[test]
    fn unstamped_and_unsignaled_is_dev() {
        assert_eq!(mode_from(None, None, None), InstallMode::Dev);
        assert_eq!(mode_from(None, None, Some("vite")), InstallMode::Dev);
    }

    #[test]
    fn stamped_without_the_image_signal_is_a_checkout() {
        assert_eq!(
            mode_from(None, None, Some("prod-server")),
            InstallMode::Checkout
        );
        assert!(
            InstallMode::Checkout
                .refusal()
                .is_some_and(|s| s.contains("git checkout"))
        );
    }
}
