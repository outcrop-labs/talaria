//! Geometry for the shell. All bounds are logical units, applied by the shell
//! on every resize — never auto_resize (tauri#10131), never physical units.

use tauri::{LogicalPosition, LogicalSize, Manager, Position, Rect, Size, Window};

use crate::{ShellState, View};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bounds {
    fn rect(self) -> Rect {
        Rect {
            position: Position::Logical(LogicalPosition::new(self.x, self.y)),
            size: Size::Logical(LogicalSize::new(self.w, self.h)),
        }
    }
}

const ZERO: Bounds = Bounds {
    x: 0.0,
    y: 0.0,
    w: 0.0,
    h: 0.0,
};

/// The browse bar's height in logical px (TALA-83): the strip of shell chrome
/// above a browser tab.
pub const BAR: f64 = 44.0;

/// (launcher-or-bar, content) bounds for a window of `w`×`h` logical px. With
/// an instance active the instance webview IS the window — the launcher is
/// hidden away (and `hide()`d besides; zero width is belt-and-suspenders) —
/// because switching lives inside the instance UI, not in shell chrome. The
/// launcher gets the whole window in welcome mode. In browse the bar gets
/// the top strip and the tab the rest (BAR).
pub fn bounds_for(view: View, w: f64, h: f64) -> (Bounds, Bounds) {
    match view {
        View::Active => (
            ZERO,
            Bounds {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
        ),
        View::Welcome => (
            Bounds {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
            ZERO,
        ),
        View::Browse => (
            Bounds {
                x: 0.0,
                y: 0.0,
                w,
                h: BAR,
            },
            Bounds {
                x: 0.0,
                y: BAR,
                w,
                h: (h - BAR).max(0.0),
            },
        ),
    }
}

/// Re-apply geometry to the launcher and the active instance webview.
///
/// Runs at the end of setup and on every `WindowEvent::Resized`. The first
/// resize can arrive before `ShellState` exists — GTK realizes the window
/// while the builder is still starting — and a relayout that no-ops there
/// is never repeated. WebKitGTK then leaves the launcher at its default
/// 1×1, off-screen (tauri#10420). An undecorated window that never paints
/// a buffer is not mapped on Wayland, so a desktop launch looks like the
/// process never started. A resize that beats state still places the
/// launcher: welcome is the only view that exists before state.
///
/// In browse the same two slots hold the browse bar (the launcher slot:
/// `main` is hidden, `browsebar` takes the strip) and the active browser
/// tab (`browtab-<id>`, the content slot).
pub fn relayout(window: &Window) {
    let (view, active, tab) = match window.app_handle().try_state::<ShellState>() {
        Some(state) => (
            *state.view.lock().unwrap(),
            state.active.lock().unwrap().clone(),
            state.active_tab.lock().unwrap().clone(),
        ),
        None => (View::Welcome, None, None),
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    // A 0×0 realize pass must not stick. The real size follows.
    if size.width == 0 || size.height == 0 {
        return;
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    let (launcher, content) =
        bounds_for(view, size.width as f64 / scale, size.height as f64 / scale);
    // The first slot is the launcher, or the browse bar over it; the second
    // the active instance, or the active browser tab.
    let (first, second) = if view == View::Browse {
        ("browsebar", tab.map(|id| format!("browtab-{id}")))
    } else {
        ("main", active.map(|id| format!("instance-{id}")))
    };
    if let Some(webview) = window.get_webview(first) {
        let _ = webview.set_bounds(launcher.rect());
    }
    if let Some(label) = second
        && let Some(webview) = window.get_webview(&label)
    {
        let _ = webview.set_bounds(content.rect());
    }
}

/// The rect a brand-new instance webview should be born at — the full window,
/// since activation means the instance becomes the window. WebviewBuilder
/// cannot create hidden, and WebKitGTK mis-places new children until a
/// set_bounds (tauri#10420), so it is born exactly where it belongs.
pub fn creation_rect(window: &Window) -> Rect {
    let Ok(size) = window.inner_size() else {
        return Rect {
            position: Position::Logical(LogicalPosition::new(0.0, 0.0)),
            size: Size::Logical(LogicalSize::new(1.0, 1.0)),
        };
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let (_, content) = bounds_for(
        View::Active,
        size.width as f64 / scale,
        size.height as f64 / scale,
    );
    content.rect()
}

/// The rect a brand-new browser tab's webview should be born at — the
/// window under the browse bar (`creation_rect`'s twin for View::Browse,
/// with the same born-exactly-where-it-belongs reason: tauri#10420).
pub fn browse_content_rect(window: &Window) -> Rect {
    let Ok(size) = window.inner_size() else {
        return Rect {
            position: Position::Logical(LogicalPosition::new(0.0, 0.0)),
            size: Size::Logical(LogicalSize::new(1.0, 1.0)),
        };
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let (_, content) = bounds_for(
        View::Browse,
        size.width as f64 / scale,
        size.height as f64 / scale,
    );
    content.rect()
}

/// The window's opening size on a work area of `avail` logical px: the
/// configured `want`, shrunk to fit when the screen is smaller (a 1280×800
/// window on a 1440×900 panel at 167 % scale is 864×540 logical, and the old
/// fixed size put the themed min/max/close off-screen), plus a minimum that
/// never exceeds what fits. Returns (size, min size).
pub fn fit(want: (f64, f64), min: (f64, f64), avail: (f64, f64)) -> ((f64, f64), (f64, f64)) {
    // Leave a little of the desktop showing, so a fitted window still reads as
    // a window and its edges can be grabbed.
    const ROOM: f64 = 0.94;
    let room = |want: f64, avail: f64| {
        if avail <= 0.0 || want <= avail * ROOM {
            want
        } else {
            (avail * ROOM).floor()
        }
    };
    let size = (room(want.0, avail.0), room(want.1, avail.1));
    (size, (min.0.min(size.0), min.1.min(size.1)))
}

/// Fit the main window to the monitor it opens on (see `fit`). Runs once in
/// setup, before the first relayout. Wayland does not let a client place its
/// window, so only the size is set; the compositor decides where it goes.
pub fn fit_to_monitor(window: &Window, want: (f64, f64), min: (f64, f64)) {
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten())
        .or_else(|| {
            window
                .available_monitors()
                .ok()
                .and_then(|m| m.into_iter().next())
        });
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let avail = (
        area.size.width as f64 / scale,
        area.size.height as f64 / scale,
    );
    let (size, min_size) = fit(want, min, avail);
    let _ = window.set_min_size(Some(Size::Logical(LogicalSize::new(
        min_size.0, min_size.1,
    ))));
    if size != want {
        let _ = window.set_size(Size::Logical(LogicalSize::new(size.0, size.1)));
        let _ = window.center();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_screen_keeps_the_configured_size() {
        assert_eq!(
            fit((1280.0, 800.0), (760.0, 480.0), (2560.0, 1440.0)),
            ((1280.0, 800.0), (760.0, 480.0))
        );
    }

    #[test]
    fn a_small_or_high_scale_screen_shrinks_the_window_and_its_minimum() {
        // 1440×900 at 167 %: 864×540 logical.
        let (size, min) = fit((1280.0, 800.0), (760.0, 480.0), (864.0, 540.0));
        assert!(size.0 <= 864.0 && size.1 <= 540.0, "{size:?}");
        assert_eq!(size, (812.0, 507.0));
        assert_eq!(min, (760.0, 480.0));
        let (size, min) = fit((1280.0, 800.0), (760.0, 480.0), (640.0, 400.0));
        assert_eq!(size, (601.0, 376.0));
        assert_eq!(min, size, "the minimum never exceeds what fits");
    }

    #[test]
    fn an_unknown_work_area_changes_nothing() {
        assert_eq!(
            fit((1280.0, 800.0), (760.0, 480.0), (0.0, 0.0)),
            ((1280.0, 800.0), (760.0, 480.0))
        );
    }

    #[test]
    fn welcome_gives_the_launcher_everything() {
        let (launcher, content) = bounds_for(View::Welcome, 1280.0, 800.0);
        assert_eq!(
            launcher,
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 1280.0,
                h: 800.0
            }
        );
        assert_eq!(content, ZERO);
    }

    #[test]
    fn an_active_instance_is_the_whole_window() {
        let (launcher, content) = bounds_for(View::Active, 640.0, 480.0);
        assert_eq!(launcher, ZERO);
        assert_eq!(
            content,
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 640.0,
                h: 480.0
            }
        );
    }

    #[test]
    fn browse_gives_the_bar_the_top_and_the_tab_the_rest() {
        let (bar, content) = bounds_for(View::Browse, 1280.0, 800.0);
        assert_eq!(
            bar,
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 1280.0,
                h: 44.0
            }
        );
        assert_eq!(
            content,
            Bounds {
                x: 0.0,
                y: 44.0,
                w: 1280.0,
                h: 756.0
            }
        );
    }
}
