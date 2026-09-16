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

/// Re-apply geometry to the launcher and the active instance webview. Runs on
/// every WindowEvent::Resized and after every view change.
pub fn relayout(window: &Window) {
    let Some(state) = window.app_handle().try_state::<ShellState>() else {
        return;
    };
    let view = *state.view.lock().unwrap();
    let active = state.active.lock().unwrap().clone();
    let Ok(size) = window.inner_size() else {
        return;
    };
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
