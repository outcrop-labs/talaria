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

/// (launcher, content) bounds for a window of `w`×`h` logical px. With an
/// instance active the instance webview IS the window — the launcher is
/// hidden away (and `hide()`d besides; zero width is belt-and-suspenders) —
/// because switching lives inside the instance UI, not in shell chrome. The
/// launcher gets the whole window in welcome mode.
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
pub fn relayout(window: &Window) {
    let (view, active) = match window.app_handle().try_state::<ShellState>() {
        Some(state) => (
            *state.view.lock().unwrap(),
            state.active.lock().unwrap().clone(),
        ),
        None => (View::Welcome, None),
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
    if let Some(webview) = window.get_webview("main") {
        let _ = webview.set_bounds(launcher.rect());
    }
    if let Some(id) = active
        && let Some(webview) = window.get_webview(&format!("instance-{id}"))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
