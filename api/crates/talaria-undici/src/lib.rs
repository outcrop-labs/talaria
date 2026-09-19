//! Map reqwest errors onto the undici-shaped sentences the TS hop used.
pub fn undici_message(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "The operation was aborted due to timeout".into()
    } else {
        "fetch failed".into()
    }
}
