//! Drag images on a HiDPI Linux display (docs/DESKTOP.md, the section of the
//! same name). Linux only: lib.rs declares this module for Linux targets, and
//! the shell's exported `gtk_drag_set_icon_surface` (main.rs) is its one caller.
//!
//! THE BUG, in WebKitGTK. A drag with no `setDragImage` shows a snapshot of the
//! dragged element, and WebKit takes it at the page's device scale: a
//! 240×120 CSS px card at scale 2 is a 480×240 px bitmap
//! (WebCore/page/FrameSnapshotting.cpp:122,136; "Later code expects the drag
//! image to be scaled by device's scale factor", WebCore/platform/DragImage.cpp:
//! 230-234). The UI process wraps those pixels in a cairo image surface whose
//! device scale is cairo's default 1 (WebKit/UIProcess/gtk/GtkUtilities.cpp:
//! 192-198), sets the grab point as its device offset and hands it to GTK
//! (WebKit/UIProcess/API/gtk/DragSourceGtk3.cpp:145-149, at 2.52.5; main is
//! unchanged). GTK sizes the icon from the surface's user-space extents
//! (gtk/gtkdnd.c `_gtk_cairo_surface_extents`, 2270-2300; the size request at
//! 2354, the hotspot = the extents' origin at 2369, GTK 3.24.52), so each
//! device pixel became one logical pixel: at a GTK scale s > 1 (200%, and the
//! fractional scales GTK3 renders at 2) the ghost was s times the web size,
//! with the cursor nearer its corner than the point grabbed. On X11 and
//! Wayland the drag is managed (gtkdnd.c:1687-1699): GTK puts that icon in
//! GDK's own drag window (2109-2110, 2124), hands GDK the hotspot (2086), and
//! GDK keeps the window at the pointer minus the hotspot
//! (gdk/x11/gdkdnd-x11.c:2185-2194, gdk/wayland/gdkdnd-wayland.c:361-377).
//!
//! macOS divides the image by the device scale (WebKit/UIProcess/mac/
//! WebViewImpl.mm:4515-4519) and Windows' WebView2 drags with Chromium's own
//! code: neither has the bug, and none of this is compiled there.
//!
//! THE FIX. WebKit's call reaches the shell first: the executable exports its
//! own `gtk_drag_set_icon_surface`, and the loader looks in the executable
//! before libgtk-3 (desktop/packaging/linux/check-drag-icon-shim.sh checks
//! both halves in CI). `set_icon_surface` stamps the web view's scale on the
//! surface — device scale (s, s), and the device offset times s so the
//! hotspot stays the CSS grab point — then calls GTK's own function, found
//! with `dlsym(RTLD_NEXT)`. GTK then reads extents of W/s × H/s: the web size,
//! drawn pixel for pixel, with the cursor on the point grabbed. A device scale
//! is how GTK documents a HiDPI surface, and the form an upstream fix in
//! DragSourceGtk3 would take (bugs.webkit.org 292058 may be this bug).
//!
//! WHICH SCALE. s must be the integer WebKit snapshotted at:
//! `gtk_widget_get_scale_factor` of the web view (WebKit/UIProcess/API/gtk/
//! WebKitWebViewBase.cpp:2540, 2550, at 2.52.5), which follows the monitor the
//! window is on. GTK keeps that widget on the drag before WebKit's
//! set-icon call (gtkdnd.c:1846; DragSourceGtk3.cpp:140 then 149), and
//! `gtk_drag_get_source_widget` returns it (gtkdnd.c:917-938). The drag
//! context's source window is not it: it is GTK's pooled 1×1 ipc popup
//! (gtkdnd.c:312-315, 1828). On X11 every window has the screen's one scale,
//! so either would do. On Wayland that popup is never mapped (it sits at
//! -99,-99; gdk/wayland/gdkwindow-wayland.c:3160-3162) and keeps monitor 0's
//! scale from its creation (831-835, 4907-4916), so on monitors at different
//! scales it would halve a ghost or leave it doubled. The popup is only the
//! fallback for a drag GTK has no source widget for.
//!
//! THE GUARDS. A drag from a web view at scale 1 is left alone. A surface that already
//! carries a device scale is left alone: a WebKitGTK fixed upstream that way,
//! or any other caller that sets one. Drag data, actions and events never
//! pass through here. Nothing here may unwind into C — Rust aborts the
//! process when a panic reaches an `extern "C"` frame — so the fitting and its
//! log line run under `catch_unwind`, and GTK gets the call whatever happens
//! to them.
//!
//! THE COST. WebKitGTK's `setDragImage(<img>)` keeps the image at its natural
//! pixels instead of scaling it by the device scale (WebCore/dom/
//! DataTransfer.cpp:612-615 → WebCore/platform/skia/DragImageSkia.cpp:97-100).
//! Such a ghost looked the web size on a HiDPI display by accident; it now
//! shows at 1/s. ui/ never calls setDragImage; an app or an agent's HTML
//! artifact that drags with an image preview will see it smaller. An element
//! passed to setDragImage is snapshotted like the default and stays right.

use std::{
    ffi::c_void,
    panic::{self, AssertUnwindSafe},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use gtk::{cairo::ffi as cairo, gdk::ffi as gdk};

/// GDK's drag context, as GTK's signature names it.
pub type Context = gdk::GdkDragContext;
/// A cairo surface, as GTK's signature names it.
pub type Surface = cairo::cairo_surface_t;
/// GTK's own `gtk_drag_set_icon_surface`.
type SetIconSurface = unsafe extern "C" fn(*mut Context, *mut Surface);

/// The device scale and offset an icon should carry, or `None` to leave it
/// as it came. `surface_scale` and `offset` are the surface's own;
/// `source_scale` is the integer scale of the web view the drag left.
fn fitted(
    surface_scale: (f64, f64),
    offset: (f64, f64),
    source_scale: i32,
) -> Option<(f64, (f64, f64))> {
    if source_scale <= 1 || surface_scale != (1.0, 1.0) {
        return None;
    }
    let s = f64::from(source_scale);
    Some((s, (offset.0 * s, offset.1 * s)))
}

/// `fitted`, applied to `surface`. True when it changed it.
///
/// # Safety
/// `surface` is a live cairo surface.
unsafe fn fit(surface: *mut Surface, source_scale: i32) -> bool {
    if unsafe { cairo::cairo_surface_status(surface) } != cairo::STATUS_SUCCESS {
        return false;
    }
    let (mut sx, mut sy, mut ox, mut oy) = (1.0, 1.0, 0.0, 0.0);
    unsafe {
        cairo::cairo_surface_get_device_scale(surface, &mut sx, &mut sy);
        cairo::cairo_surface_get_device_offset(surface, &mut ox, &mut oy);
    }
    let Some((s, (x, y))) = fitted((sx, sy), (ox, oy), source_scale) else {
        return false;
    };
    unsafe {
        cairo::cairo_surface_set_device_scale(surface, s, s);
        cairo::cairo_surface_set_device_offset(surface, x, y);
    }
    true
}

/// The integer scale WebKit snapshotted the image at, and where it was read
/// (for the log line): the scale of the widget the drag left, the web view
/// (the header's "WHICH SCALE"). Without a source widget, the drag's source
/// window's, which is right on X11 only; without either, 1.
///
/// # Safety
/// `context` is the source side's live drag context.
unsafe fn source_scale(context: *mut Context) -> (i32, &'static str) {
    let widget = unsafe { gtk::ffi::gtk_drag_get_source_widget(context) };
    if !widget.is_null() {
        return (
            unsafe { gtk::ffi::gtk_widget_get_scale_factor(widget) },
            "the dragged widget",
        );
    }
    let window = unsafe { gdk::gdk_drag_context_get_source_window(context) };
    if window.is_null() {
        return (1, "no source");
    }
    (
        unsafe { gdk::gdk_window_get_scale_factor(window) },
        "the drag's source window",
    )
}

/// GTK's own `gtk_drag_set_icon_surface`: the next definition after the
/// executable's in the loader's search order. That is libgtk-3's, or an
/// LD_PRELOAD library's that chains to it the same way.
fn gtk_own() -> Option<SetIconSurface> {
    static OWN: OnceLock<Option<SetIconSurface>> = OnceLock::new();
    *OWN.get_or_init(|| {
        let found = unsafe { libc::dlsym(libc::RTLD_NEXT, c"gtk_drag_set_icon_surface".as_ptr()) };
        if found.is_null() {
            None
        } else {
            // SAFETY: the symbol is GTK's gtk_drag_set_icon_surface, whose C
            // signature is exactly SetIconSurface's.
            Some(unsafe { std::mem::transmute::<*mut c_void, SetIconSurface>(found) })
        }
    })
}

/// One line per run for each, in the shell's log: enough to see from a
/// person's log file that the fix ran, at which scale, on which WebKitGTK.
static SAID_FITTED: AtomicBool = AtomicBool::new(false);
static SAID_MISSING: AtomicBool = AtomicBool::new(false);

/// Everything before GTK's call: may panic only through the log line, and
/// `set_icon_surface` catches that.
///
/// # Safety
/// As `set_icon_surface`.
unsafe fn prepare(context: *mut Context, surface: *mut Surface) {
    if context.is_null() || surface.is_null() {
        return;
    }
    let (scale, from) = unsafe { source_scale(context) };
    if unsafe { fit(surface, scale) } && !SAID_FITTED.swap(true, Ordering::Relaxed) {
        let (major, minor, micro) = unsafe {
            (
                webkit2gtk::ffi::webkit_get_major_version(),
                webkit2gtk::ffi::webkit_get_minor_version(),
                webkit2gtk::ffi::webkit_get_micro_version(),
            )
        };
        eprintln!(
            "talaria-desktop: drag images: WebKitGTK {major}.{minor}.{micro} hands GTK device \
             pixels; shown at scale {scale} (read from {from}), the web size (drag_icon.rs)"
        );
    }
}

/// What the shell's exported `gtk_drag_set_icon_surface` does (main.rs): fit
/// the icon to the web size when it needs it, then hand the call to GTK,
/// always, with the same arguments.
///
/// # Safety
/// GTK's contract for `gtk_drag_set_icon_surface`: on the GTK main thread,
/// `context` is the source side's live drag context and `surface` a live
/// cairo surface (GTK itself refuses a null one, with a warning).
pub unsafe fn set_icon_surface(context: *mut Context, surface: *mut Surface) {
    let _ = panic::catch_unwind(AssertUnwindSafe(|| unsafe { prepare(context, surface) }));
    match gtk_own() {
        Some(own) => unsafe { own(context, surface) },
        None => {
            let _ = panic::catch_unwind(|| {
                if !SAID_MISSING.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "talaria-desktop: drag images: GTK's gtk_drag_set_icon_surface was not \
                         found; drags keep GTK's default icon (drag_icon.rs)"
                    );
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::*;

    #[test]
    fn a_scale_one_window_leaves_the_icon_alone() {
        assert_eq!(fitted((1.0, 1.0), (-30.0, -10.0), 1), None);
        assert_eq!(fitted((1.0, 1.0), (-30.0, -10.0), 0), None);
    }

    #[test]
    fn a_surface_that_already_has_a_scale_is_left_alone() {
        assert_eq!(fitted((2.0, 2.0), (-60.0, -20.0), 2), None);
        assert_eq!(fitted((1.0, 2.0), (-30.0, -20.0), 2), None);
    }

    #[test]
    fn the_offset_scales_with_the_window() {
        assert_eq!(
            fitted((1.0, 1.0), (-30.0, -10.0), 2),
            Some((2.0, (-60.0, -20.0)))
        );
        assert_eq!(
            fitted((1.0, 1.0), (-30.0, -10.0), 3),
            Some((3.0, (-90.0, -30.0)))
        );
    }

    /// A cairo image surface `w`×`h` px with `offset` as its device offset,
    /// as WebKit hands it to GTK (the offset is minus the grab point).
    fn webkit_surface(w: i32, h: i32, offset: (f64, f64)) -> *mut Surface {
        unsafe {
            let surface = cairo::cairo_image_surface_create(cairo::FORMAT_A_RGB32, w, h);
            assert_eq!(cairo::cairo_surface_status(surface), cairo::STATUS_SUCCESS);
            cairo::cairo_surface_set_device_offset(surface, offset.0, offset.1);
            surface
        }
    }

    fn scale_and_offset(surface: *mut Surface) -> ((f64, f64), (f64, f64)) {
        let (mut sx, mut sy, mut ox, mut oy) = (0.0, 0.0, 0.0, 0.0);
        unsafe {
            cairo::cairo_surface_get_device_scale(surface, &mut sx, &mut sy);
            cairo::cairo_surface_get_device_offset(surface, &mut ox, &mut oy);
        }
        ((sx, sy), (ox, oy))
    }

    /// GTK 3.24's `_gtk_cairo_surface_extents` (gtkdnd.c:2270-2300): the
    /// surface's clip in user space, rounded out. gtk_drag_set_icon_surface
    /// sizes the icon by its width and height and takes its origin as the
    /// hotspot.
    fn gtk_extents(surface: *mut Surface) -> (f64, f64, f64, f64) {
        let (mut x1, mut y1, mut x2, mut y2) = (0.0, 0.0, 0.0, 0.0);
        unsafe {
            let cr = cairo::cairo_create(surface);
            cairo::cairo_clip_extents(cr, &mut x1, &mut y1, &mut x2, &mut y2);
            cairo::cairo_destroy(cr);
        }
        (x1.floor(), y1.floor(), x2.ceil(), y2.ceil())
    }

    #[test]
    fn webkit_s_icon_at_scale_two_is_the_web_size_and_keeps_its_grab_point() {
        // A 200×60 CSS px element grabbed at (30, 10), snapshotted at scale 2.
        let surface = webkit_surface(400, 120, (-30.0, -10.0));
        // Today, GTK reads a 400×120 icon: twice the web size.
        assert_eq!(gtk_extents(surface), (30.0, 10.0, 430.0, 130.0));
        assert!(unsafe { fit(surface, 2) });
        assert_eq!(scale_and_offset(surface), ((2.0, 2.0), (-60.0, -20.0)));
        // 200×60 logical px, hotspot still (30, 10): the CSS size and grab.
        assert_eq!(gtk_extents(surface), (30.0, 10.0, 230.0, 70.0));
        // A second pass changes nothing: the surface has a scale now.
        assert!(!unsafe { fit(surface, 2) });
        assert_eq!(scale_and_offset(surface), ((2.0, 2.0), (-60.0, -20.0)));
        unsafe { cairo::cairo_surface_destroy(surface) };
    }

    #[test]
    fn a_scale_one_window_changes_nothing_on_the_surface() {
        let surface = webkit_surface(200, 60, (-30.0, -10.0));
        assert!(!unsafe { fit(surface, 1) });
        assert_eq!(scale_and_offset(surface), ((1.0, 1.0), (-30.0, -10.0)));
        assert_eq!(gtk_extents(surface), (30.0, 10.0, 230.0, 70.0));
        unsafe { cairo::cairo_surface_destroy(surface) };
    }

    #[test]
    fn an_icon_that_already_has_a_scale_is_left_alone() {
        // What an upstream fix in DragSourceGtk3 would hand over.
        let surface = webkit_surface(400, 120, (-60.0, -20.0));
        unsafe { cairo::cairo_surface_set_device_scale(surface, 2.0, 2.0) };
        assert!(!unsafe { fit(surface, 2) });
        assert_eq!(scale_and_offset(surface), ((2.0, 2.0), (-60.0, -20.0)));
        assert_eq!(gtk_extents(surface), (30.0, 10.0, 230.0, 70.0));
        unsafe { cairo::cairo_surface_destroy(surface) };
    }

    #[test]
    fn a_surface_in_an_error_state_is_left_alone() {
        // cairo refuses a negative size with an error surface, not a crash.
        let surface = unsafe { cairo::cairo_image_surface_create(cairo::FORMAT_A_RGB32, -1, -1) };
        assert_ne!(
            unsafe { cairo::cairo_surface_status(surface) },
            cairo::STATUS_SUCCESS
        );
        assert!(!unsafe { fit(surface, 2) });
        unsafe { cairo::cairo_surface_destroy(surface) };
    }

    /// The call goes on to GTK's own function, never back into the shell's.
    /// (This test binary has no shim; the shell's main.rs does, and the CI
    /// check script proves WebKit binds to it.)
    #[test]
    fn the_call_is_forwarded_to_libgtk() {
        // A live call into libgtk-3, so the linker keeps it in this test
        // binary (--as-needed drops a library nothing live references); the
        // shell itself is full of them.
        assert_eq!(unsafe { gtk::ffi::gtk_get_major_version() }, 3);
        let own = gtk_own().expect("RTLD_NEXT finds gtk_drag_set_icon_surface");
        let mut info = libc::Dl_info {
            dli_fname: std::ptr::null(),
            dli_fbase: std::ptr::null_mut(),
            dli_sname: std::ptr::null(),
            dli_saddr: std::ptr::null_mut(),
        };
        assert_ne!(unsafe { libc::dladdr(own as *const c_void, &mut info) }, 0);
        let file = unsafe { CStr::from_ptr(info.dli_fname) }.to_string_lossy();
        assert!(file.contains("libgtk-3"), "forwarded into {file}");
    }
}
