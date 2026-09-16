//! Geometry for the shell. All bounds are logical units, applied by the shell
//! on every resize — never auto_resize (tauri#10131), never physical units.

use tauri::{LogicalPosition, LogicalSize, Manager, Position, Rect, Size, Window};

use crate::ShellState;

/// Sidebar width in logical px — pure geometry; the launcher derives its own
/// layout mode from app state, never from viewport width.
pub const SIDEBAR_WIDTH: f64 = 240.0;

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

/// (launcher, content) bounds for a window of `w`×`h` logical px. With an
/// active instance the launcher is a fixed strip and content takes the rest;
/// otherwise the launcher is the whole window and content is unused.
pub fn bounds_for(active: bool, w: f64, h: f64) -> (Bounds, Bounds) {
    if active {
        (
            Bounds {
                x: 0.0,
                y: 0.0,
                w: SIDEBAR_WIDTH,
                h,
            },
            Bounds {
                x: SIDEBAR_WIDTH,
                y: 0.0,
                w: (w - SIDEBAR_WIDTH).max(0.0),
                h,
            },
        )
    } else {
        (
            Bounds {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
        )
    }
}

/// Re-apply geometry to the launcher and the active instance webview. Runs on
/// every WindowEvent::Resized and after every mode change.
pub fn relayout(window: &Window) {
    let Some(state) = window.app_handle().try_state::<ShellState>() else {
        return;
    };
    let active = state.active.lock().unwrap().clone();
    let Ok(size) = window.inner_size() else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let (launcher, content) = bounds_for(
        active.is_some(),
        size.width as f64 / scale,
        size.height as f64 / scale,
    );
    if let Some(webview) = window.get_webview("main") {
        let _ = webview.set_bounds(launcher.rect());
    }
    if let Some(id) = active
        && let Some(webview) = window.get_webview(&format!("instance-{id}"))
    {
        let _ = webview.set_bounds(content.rect());
    }
}

/// The rect a brand-new instance webview should be born at — the content area
/// as if it were already active. WebviewBuilder cannot create hidden, so the
/// only way to avoid a flash of misplaced content is to place it correctly at
/// creation (then relayout re-asserts the same bounds, tauri#10420).
pub fn creation_rect(window: &Window) -> Rect {
    let Ok(size) = window.inner_size() else {
        return Rect {
            position: Position::Logical(LogicalPosition::new(0.0, 0.0)),
            size: Size::Logical(LogicalSize::new(1.0, 1.0)),
        };
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let (_, content) = bounds_for(true, size.width as f64 / scale, size.height as f64 / scale);
    content.rect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_mode_gives_the_launcher_everything() {
        let (launcher, content) = bounds_for(false, 1280.0, 800.0);
        assert_eq!(
            launcher,
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 1280.0,
                h: 800.0
            }
        );
        assert_eq!(
            content,
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0
            }
        );
    }

    #[test]
    fn instance_mode_strips_a_fixed_sidebar() {
        let (launcher, content) = bounds_for(true, 1280.0, 800.0);
        assert_eq!(
            launcher,
            Bounds {
                x: 0.0,
                y: 0.0,
                w: SIDEBAR_WIDTH,
                h: 800.0
            }
        );
        assert_eq!(
            content,
            Bounds {
                x: SIDEBAR_WIDTH,
                y: 0.0,
                w: 1040.0,
                h: 800.0
            }
        );
    }

    #[test]
    fn content_never_goes_negative_below_min_width() {
        let (_, content) = bounds_for(true, SIDEBAR_WIDTH / 2.0, 600.0);
        assert_eq!(content.w, 0.0);
        assert_eq!(content.x, SIDEBAR_WIDTH);
    }
}
