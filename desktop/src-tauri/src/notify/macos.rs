//! macOS: UNUserNotificationCenter, with a delegate of our own.
//!
//! The legacy NSUserNotificationCenter (notify-rust's default, deprecated
//! since macOS 11) had no authorization request, delivered inside a waiter
//! whose error was discarded — so a banner macOS dropped read as a success —
//! and swizzled `[NSBundle bundleIdentifier]` process-wide (D5). This backend
//! replaces it in every bundled run:
//!
//!   * Authorization is asked only from a click (the launcher card, Settings
//!     → Desktop), never at launch. A post while it is not granted is an
//!     error that names the state and the fix; an `add` macOS refuses is an
//!     error too.
//!   * The delegate answers `willPresent` with banner + list + sound, because
//!     with close-to-background Talaria is often the active app with its
//!     window hidden, and macOS hides banners for the active app otherwise.
//!   * Every click comes back through the delegate by request identifier,
//!     `"<label>/<tag>"`, so it routes to its instance whichever thread or
//!     process posted it. A re-post of the same pair replaces the banner and
//!     shows it again (a DM re-alert); the thread identifier groups an
//!     instance's banners and the subtitle names it.
//!
//! mac-usernotifications (notify-rust's `preview-macos-un`) was read for
//! this: it does implement `willPresent`, but routes a click only to a
//! per-banner waiter registered at send time, so a click after a re-post or
//! on an older Notification Center entry goes nowhere — and it would add a
//! crate. objc2-user-notifications is already in the lock.
//!
//! A read elsewhere removes a banner (I-16): its identifier is the same
//! `"<label>/<tag>"`, so `removeDeliveredNotificationsWithIdentifiers` takes
//! exactly the caller's own, from Notification Center's list too. The OS
//! idle time (I-22) is CoreGraphics' seconds since the last input event of
//! any kind, for the whole login session.
//!
//! UN needs a real `.app` with a bundle identifier; outside one,
//! `currentNotificationCenter` throws. An unbundled dev run (`tauri dev`, a
//! bare binary) falls back to the legacy center through mac-notification-sys,
//! attributed to Terminal, and says so in its status.

use std::{
    ptr::NonNull,
    sync::{Mutex, OnceLock, mpsc},
    time::Duration,
};

use block2::RcBlock;
use objc2::{
    AnyThread, define_class, msg_send, rc::Retained, runtime::Bool, runtime::ProtocolObject,
};
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use tauri::{AppHandle, Manager};

use super::{OsStatus, Permission, Post};

/// What the status and the self-test call the unbundled fallback.
pub const LEGACY: &str = "ns-legacy (unbundled dev run)";

/// System Settings → Notifications, where a denied app is turned back on.
const SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.Notifications-Settings.extension";

const DISMISS_ACTION: &str = "com.apple.UNNotificationDismissActionIdentifier";

/// How long macOS may take to answer a settings read or an add.
const ANSWER_WAIT: Duration = Duration::from_secs(5);
/// The authorization prompt waits on the person.
const PROMPT_WAIT: Duration = Duration::from_secs(300);

/// A real `.app` with a bundle identifier. A bare binary has an identifier
/// too (Tauri embeds its Info.plist), but no bundle around it, and UN throws
/// there.
pub fn bundled() -> bool {
    let bundle = NSBundle::mainBundle();
    bundle.bundleIdentifier().is_some() && bundle.bundlePath().to_string().ends_with(".app")
}

fn bundle_id() -> Option<String> {
    NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|id| id.to_string())
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "TalariaNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Show the banner even while Talaria is the active app: with its
        /// window closed to the menu bar, it often is.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        /// A click (or the default action from Notification Center): route it
        /// by its `"<label>/<tag>"` identifier.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let id = response.notification().request().identifier().to_string();
            let action = response.actionIdentifier().to_string();
            if action != DISMISS_ACTION
                && let Some((label, tag)) = id.split_once('/')
            {
                super::clicked_tag(label, tag);
            }
            completion_handler.call(());
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

fn center() -> Retained<UNUserNotificationCenter> {
    UNUserNotificationCenter::currentNotificationCenter()
}

/// Installed once, before the first post, so a click on any banner — this
/// run's or an earlier one's — reaches it.
fn install_delegate() {
    static DELEGATE: OnceLock<Retained<Delegate>> = OnceLock::new();
    DELEGATE.get_or_init(|| {
        let delegate = Delegate::new();
        center().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        delegate
    });
}

pub fn init(_app: &AppHandle) {
    if bundled() {
        install_delegate();
    } else {
        eprintln!(
            "talaria-desktop: not a bundled app; notifications use the legacy center, attributed to Terminal"
        );
    }
}

/// The app's authorization, as macOS reports it now. Never prompts.
fn authorization() -> Result<UNAuthorizationStatus, String> {
    let (tx, rx) = mpsc::sync_channel::<UNAuthorizationStatus>(1);
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: macOS hands the completion a valid settings object.
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(status);
        }
    });
    center().getNotificationSettingsWithCompletionHandler(&block);
    rx.recv_timeout(ANSWER_WAIT).map_err(|_| {
        format!(
            "macOS did not report the notification settings in {} s",
            ANSWER_WAIT.as_secs()
        )
    })
}

fn authorization_word(status: UNAuthorizationStatus) -> &'static str {
    match status {
        UNAuthorizationStatus::NotDetermined => "notDetermined",
        UNAuthorizationStatus::Denied => "denied",
        UNAuthorizationStatus::Authorized => "authorized",
        UNAuthorizationStatus::Provisional => "provisional",
        UNAuthorizationStatus::Ephemeral => "ephemeral",
        _ => "unknown",
    }
}

fn permission_of(status: UNAuthorizationStatus) -> Permission {
    match status {
        UNAuthorizationStatus::Authorized
        | UNAuthorizationStatus::Provisional
        | UNAuthorizationStatus::Ephemeral => Permission::Granted,
        UNAuthorizationStatus::Denied => Permission::Denied,
        UNAuthorizationStatus::NotDetermined => Permission::NotDetermined,
        _ => Permission::Unavailable,
    }
}

/// The system prompt. Only ever from a click.
fn request_authorization() -> Result<bool, String> {
    let (tx, rx) = mpsc::sync_channel::<Result<bool, String>>(1);
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |granted: Bool, err: *mut NSError| {
        // SAFETY: a null or valid NSError, as the completion's contract says.
        let result = match unsafe { err.as_ref() } {
            Some(err) => Err(err.localizedDescription().to_string()),
            None => Ok(granted.as_bool()),
        };
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(result);
        }
    });
    center().requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge,
        &block,
    );
    rx.recv_timeout(PROMPT_WAIT)
        .map_err(|_| "the notification prompt was not answered".to_string())?
}

/// Hand one request to Notification Center; its refusal is the error.
fn add(identifier: &str, post: &Post) -> Result<(), String> {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&post.title));
    if !post.instance.is_empty() {
        content.setSubtitle(&NSString::from_str(&post.instance));
    }
    content.setBody(&NSString::from_str(post.body.as_deref().unwrap_or("")));
    content.setThreadIdentifier(&NSString::from_str(&post.label));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(identifier),
        &content,
        None,
    );
    let (tx, rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |err: *mut NSError| {
        // SAFETY: a null or valid NSError, as the completion's contract says.
        let result = match unsafe { err.as_ref() } {
            Some(err) => Err(format!(
                "macOS refused the notification: {}",
                err.localizedDescription()
            )),
            None => Ok(()),
        };
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(result);
        }
    });
    center().addNotificationRequest_withCompletionHandler(&request, Some(&block));
    rx.recv_timeout(ANSWER_WAIT).map_err(|_| {
        format!(
            "Notification Center did not accept the notification in {} s",
            ANSWER_WAIT.as_secs()
        )
    })?
}

/// A bundled post: refused, in words, unless macOS has said yes.
fn post_un(post: &Post) -> Result<(), String> {
    install_delegate();
    match authorization()? {
        UNAuthorizationStatus::Authorized
        | UNAuthorizationStatus::Provisional
        | UNAuthorizationStatus::Ephemeral => {}
        UNAuthorizationStatus::NotDetermined => {
            return Err(
                "not allowed yet (notDetermined): choose Allow notifications in Settings → Desktop"
                    .into(),
            );
        }
        UNAuthorizationStatus::Denied => {
            return Err(
                "blocked (denied): turn Talaria on in System Settings → Notifications".into(),
            );
        }
        other => {
            return Err(format!(
                "macOS reports an authorization this build does not know ({})",
                other.0
            ));
        }
    }
    add(&format!("{}/{}", post.label, post.tag), post)
}

/// The unbundled dev run: the legacy center, on a thread of its own that
/// waits for the click. Attribution is Terminal's.
fn post_legacy(post: &Post) {
    let title = post.title.clone();
    let subtitle = post.instance.clone();
    let body = post.body.clone().unwrap_or_default();
    let label = post.label.clone();
    let tag = post.tag.clone();
    std::thread::spawn(move || {
        // Errs "already set" after the first call; the first one counts.
        let _ = mac_notification_sys::set_application("com.apple.Terminal");
        let mut notification = mac_notification_sys::Notification::new();
        notification
            .title(&title)
            .subtitle(&subtitle)
            .message(&body)
            .wait_for_click(true);
        match notification.send() {
            Ok(mac_notification_sys::NotificationResponse::Click) => {
                super::clicked_tag(&label, &tag);
            }
            Ok(_) => {}
            Err(e) => eprintln!("talaria-desktop: {label}: the legacy center: {e}"),
        }
    });
}

pub async fn post(post: &Post, _previous: Option<u32>) -> Result<u32, String> {
    if !bundled() {
        post_legacy(post);
        return Ok(0);
    }
    let post = post.clone();
    tauri::async_runtime::spawn_blocking(move || post_un(&post))
        .await
        .map_err(|e| format!("posting the notification: {e}"))??;
    Ok(0)
}

/// Remove these banners — delivered, and any still pending — by their
/// `"<label>/<tag>"` identifiers. The legacy center (an unbundled dev run)
/// cannot: it keeps them. macOS answers nothing, so the count is the
/// banners this run had posted.
pub async fn close(label: &str, tags: &[String], entries: &[super::Entry]) -> u32 {
    if tags.is_empty() || !bundled() {
        return 0;
    }
    let ids: Vec<String> = tags.iter().map(|tag| format!("{label}/{tag}")).collect();
    let removed = tauri::async_runtime::spawn_blocking(move || remove(&ids)).await;
    match removed {
        Ok(()) => {
            eprintln!("talaria-desktop: {label}: removed {} banner(s)", tags.len());
            u32::try_from(entries.len()).unwrap_or(u32::MAX)
        }
        Err(e) => {
            eprintln!("talaria-desktop: {label}: removing banners: {e}");
            0
        }
    }
}

/// Remove delivered and pending requests by identifier.
fn remove(ids: &[String]) {
    let ids: Vec<Retained<NSString>> = ids.iter().map(|id| NSString::from_str(id)).collect();
    let ids = NSArray::from_retained_slice(&ids);
    let center = center();
    center.removeDeliveredNotificationsWithIdentifiers(&ids);
    center.removePendingNotificationRequestsWithIdentifiers(&ids);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// `CGEventSourceSecondsSinceLastEventType(CGEventSourceStateID,
    /// CGEventType)`.
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event: u32) -> f64;
}

/// `kCGEventSourceStateCombinedSessionState`: every input source of the
/// login session.
const COMBINED_SESSION_STATE: i32 = 0;
/// `kCGAnyInputEventType` (`~0`).
const ANY_INPUT_EVENT: u32 = u32::MAX;

/// Milliseconds since the last input of any kind in this login session.
pub async fn os_idle() -> Option<u64> {
    // SAFETY: a pure query with two plain values; no pointers.
    let secs =
        unsafe { CGEventSourceSecondsSinceLastEventType(COMBINED_SESSION_STATE, ANY_INPUT_EVENT) };
    (secs.is_finite() && secs >= 0.0).then(|| (secs * 1000.0) as u64)
}

fn legacy_status() -> OsStatus {
    OsStatus {
        backend: LEGACY.into(),
        permission: Permission::Granted,
        server: None,
        capabilities: None,
        detail: Some(
            "an unbundled dev run: banners go through the legacy center, attributed to Terminal"
                .into(),
        ),
    }
}

fn status_now() -> OsStatus {
    if !bundled() {
        return legacy_status();
    }
    match authorization() {
        Ok(status) => OsStatus {
            backend: "un".into(),
            permission: permission_of(status),
            server: None,
            capabilities: None,
            detail: Some(format!(
                "{} ({})",
                bundle_id().unwrap_or_default(),
                authorization_word(status)
            )),
        },
        Err(e) => OsStatus {
            backend: "un".into(),
            permission: Permission::Unavailable,
            server: None,
            capabilities: None,
            detail: Some(format!("unavailable: {e}")),
        },
    }
}

pub async fn probe() -> OsStatus {
    tauri::async_runtime::spawn_blocking(status_now)
        .await
        .unwrap_or_else(|e| OsStatus {
            backend: "un".into(),
            permission: Permission::Unavailable,
            server: None,
            capabilities: None,
            detail: Some(format!("unavailable: {e}")),
        })
}

/// From a click: the prompt when macOS has not asked yet, System Settings
/// when the person said no before.
pub async fn request() -> OsStatus {
    let asked = tauri::async_runtime::spawn_blocking(|| {
        if !bundled() {
            return;
        }
        install_delegate();
        match authorization() {
            Ok(UNAuthorizationStatus::NotDetermined) => match request_authorization() {
                Ok(granted) => eprintln!("talaria-desktop: notifications allowed: {granted}"),
                Err(e) => eprintln!("talaria-desktop: the notification prompt: {e}"),
            },
            Ok(UNAuthorizationStatus::Denied) => {
                if let Err(e) = std::process::Command::new("open")
                    .arg(SETTINGS_URL)
                    .status()
                {
                    eprintln!("talaria-desktop: opening System Settings: {e}");
                }
            }
            _ => {}
        }
    })
    .await;
    if let Err(e) = asked {
        eprintln!("talaria-desktop: the notification request: {e}");
    }
    probe().await
}

/// The dock badge: the sum of every instance's unread, cleared at 0.
pub fn set_badge(app: &AppHandle, total: u32) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_window("main") {
            let count = (total > 0).then_some(i64::from(total));
            if let Err(e) = window.set_badge_count(count) {
                eprintln!("talaria-desktop: the dock badge: {e}");
            }
        }
    });
}

/// The self-test (desktop-package.yml): the backend, the bundle id, the
/// authorization, and a post that must agree with it — an error unless
/// authorized. No window, no prompt.
pub fn selftest() -> serde_json::Value {
    if !bundled() {
        return serde_json::json!({
            "backend": LEGACY,
            "bundleId": bundle_id(),
            "authorization": "n/a",
            "post": "skipped: the legacy center waits on a run loop this self-test does not run",
            "close": "skipped: the legacy center cannot remove a banner",
            "osIdle": tauri::async_runtime::block_on(os_idle()),
            "background": "menu-bar",
        });
    }
    install_delegate();
    let authorization = match authorization() {
        Ok(status) => authorization_word(status).to_string(),
        Err(e) => format!("error: {e}"),
    };
    let post = Post {
        label: "main".into(),
        instance: "Talaria".into(),
        title: super::TEST_TITLE.into(),
        body: Some("the self-test".into()),
        tag: "selftest".into(),
        href: None,
        kind: None,
        renotify: false,
        source: super::Source::Page,
    };
    let posted = match post_un(&post) {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("error: {e}"),
    };
    // The removal the read-sync uses, on the self-test's own identifier.
    // UN's remove answers nothing: a call that returns is "ok".
    remove(&["main/selftest".to_string()]);
    let close = "ok";
    serde_json::json!({
        "backend": "un",
        "bundleId": bundle_id(),
        "authorization": authorization,
        "post": posted,
        "close": close,
        "osIdle": tauri::async_runtime::block_on(os_idle()),
        "background": "menu-bar",
    })
}
