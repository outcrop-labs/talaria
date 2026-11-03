//! The shell's log file. Every `eprintln!` in the crate (lib.rs defines the
//! macro) goes to stderr, and — once setup has called `open` — to
//! `talaria-desktop.log` in the platform's log directory:
//!
//! - Windows: `%LOCALAPPDATA%\app.talaria.desktop\logs\`
//! - macOS: `~/Library/Logs/app.talaria.desktop/`
//! - Linux: `~/.local/share/app.talaria.desktop/logs/`
//!
//! A release build on Windows has no console, and a Finder, Start-menu or
//! `.desktop` launch throws stderr away; this file is what a person can send.
//!
//! Each line is stamped with UTC time to the millisecond and written with one
//! `write_all` on an unbuffered file, so a crash or a kill loses nothing that
//! was already said. At `MAX_BYTES` the file becomes `talaria-desktop.log.1`,
//! replacing the one before, so the two never hold much more than 2 MB. A
//! failure to write turns the file off for the rest of the run, with one line
//! on stderr, and never stops the shell. Nothing here may call the crate's
//! `eprintln!` (it would take the lock it is holding): `::std::eprintln!` only.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

pub const FILE_NAME: &str = "talaria-desktop.log";

/// About 1 MB a file, two files.
pub const MAX_BYTES: u64 = 1024 * 1024;

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// Start mirroring to `dir/talaria-desktop.log`.
pub fn open(dir: &Path) {
    match Sink::open(dir, MAX_BYTES) {
        Ok(sink) => {
            let path = sink.path.clone();
            *SINK.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
            line(format_args!(
                "talaria-desktop: logging to {}",
                path.display()
            ));
        }
        Err(e) => ::std::eprintln!(
            "talaria-desktop: no log file in {}: {e}; logging to stderr only",
            dir.display()
        ),
    }
}

/// A panic, for the panic hook (lib.rs): the file only — the default hook
/// that runs next prints it to stderr — and never waiting on the lock, which
/// the panicking thread may be holding.
pub fn panic_line(args: std::fmt::Arguments<'_>) {
    let text = args.to_string();
    let mut sink = match SINK.try_lock() {
        Ok(sink) => sink,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    if let Some(open) = sink.as_mut() {
        let _ = open.write(SystemTime::now(), &text);
    }
}

/// One line: stderr, then the file if it is open.
pub fn line(args: std::fmt::Arguments<'_>) {
    let text = args.to_string();
    ::std::eprintln!("{text}");
    let mut sink = SINK.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(open) = sink.as_mut()
        && let Err(e) = open.write(SystemTime::now(), &text)
    {
        ::std::eprintln!(
            "talaria-desktop: the log file {} stopped: {e}",
            open.path.display()
        );
        *sink = None;
    }
}

struct Sink {
    path: PathBuf,
    file: File,
    len: u64,
    max: u64,
}

impl Sink {
    fn open(dir: &Path, max: u64) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(FILE_NAME);
        if fs::metadata(&path).is_ok_and(|m| m.len() >= max) {
            rotate(&path)?;
        }
        let file = append(&path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            path,
            file,
            len,
            max,
        })
    }

    fn write(&mut self, now: SystemTime, text: &str) -> io::Result<()> {
        let line = format!("{} {text}\n", stamp(now));
        let size = line.len() as u64;
        if self.len > 0 && self.len + size > self.max {
            // A second Talaria process (Windows has no single-instance guard
            // yet) may have rotated the file already, leaving this handle on
            // the old one: then follow it to the new file.
            let on_disk = fs::metadata(&self.path).map(|m| m.len()).ok();
            match on_disk {
                Some(n) if n < self.len && n + size <= self.max => {
                    self.file = append(&self.path)?;
                    self.len = n;
                }
                _ => {
                    // Renaming an open file is fine on Windows too: std opens
                    // files with FILE_SHARE_DELETE. If it fails anyway, start
                    // this file over rather than stop. A fresh handle, because
                    // an append-only one cannot truncate on Windows.
                    self.file = match rotate(&self.path) {
                        Ok(()) => append(&self.path)?,
                        Err(_) => OpenOptions::new()
                            .create(true)
                            .write(true)
                            .truncate(true)
                            .open(&self.path)?,
                    };
                    self.len = 0;
                }
            }
        }
        self.file.write_all(line.as_bytes())?;
        self.len += size;
        Ok(())
    }
}

fn append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// `talaria-desktop.log` → `talaria-desktop.log.1`, replacing it.
fn rotate(path: &Path) -> io::Result<()> {
    let mut previous = path.as_os_str().to_owned();
    previous.push(".1");
    fs::rename(path, previous)
}

/// `2026-11-02T19:04:05.123Z`.
fn stamp(now: SystemTime) -> String {
    let ms = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let (days, of_day) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3_600_000,
        of_day / 60_000 % 60,
        of_day / 1000 % 60,
        of_day % 1000
    )
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian
/// calendar (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "talaria-logfile-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stamps_are_utc_iso_to_the_millisecond() {
        assert_eq!(stamp(at(0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(stamp(at(1_793_646_245_123)), "2026-11-02T19:04:05.123Z");
        assert_eq!(stamp(at(1_709_251_199_999)), "2024-02-29T23:59:59.999Z");
        assert_eq!(stamp(at(951_868_800_000)), "2000-03-01T00:00:00.000Z");
    }

    #[test]
    fn lines_append_with_a_stamp() {
        let dir = scratch("append");
        let mut sink = Sink::open(&dir, MAX_BYTES).unwrap();
        sink.write(at(0), "talaria-desktop: one").unwrap();
        sink.write(at(1), "talaria-desktop: two").unwrap();
        drop(sink);
        // A second run appends to the same file.
        let mut sink = Sink::open(&dir, MAX_BYTES).unwrap();
        sink.write(at(2), "talaria-desktop: three").unwrap();
        let text = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert_eq!(
            text,
            "1970-01-01T00:00:00.000Z talaria-desktop: one\n\
             1970-01-01T00:00:00.001Z talaria-desktop: two\n\
             1970-01-01T00:00:00.002Z talaria-desktop: three\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_file_rotates_to_one_previous() {
        let dir = scratch("rotate");
        // Each line is 50 bytes: four fit in 200.
        let mut sink = Sink::open(&dir, 200).unwrap();
        for n in 0..20 {
            sink.write(at(n), &format!("talaria-desktop: line {n:02}"))
                .unwrap();
        }
        let read = |name: &str| fs::read_to_string(dir.join(name)).unwrap();
        let tails = |text: String| -> Vec<String> {
            text.lines()
                .map(|l| l.rsplit(' ').next().unwrap().to_string())
                .collect()
        };
        assert_eq!(tails(read(FILE_NAME)), ["16", "17", "18", "19"]);
        assert_eq!(
            tails(read(&format!("{FILE_NAME}.1"))),
            ["12", "13", "14", "15"]
        );
        assert!(!dir.join(format!("{FILE_NAME}.2")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_another_process_rotated_is_followed() {
        let dir = scratch("follow");
        let mut sink = Sink::open(&dir, 200).unwrap();
        for n in 0..3 {
            sink.write(at(n), &format!("talaria-desktop: line {n:02}"))
                .unwrap();
        }
        // The other process rotates: our file becomes .1, a new one starts.
        let path = dir.join(FILE_NAME);
        rotate(&path).unwrap();
        fs::write(&path, b"").unwrap();
        // The next line that would not fit follows the new file instead of
        // rotating it away.
        for n in 3..5 {
            sink.write(at(n), &format!("talaria-desktop: line {n:02}"))
                .unwrap();
        }
        let current = fs::read_to_string(&path).unwrap();
        assert!(current.ends_with("talaria-desktop: line 04\n"), "{current}");
        let previous = fs::read_to_string(dir.join(format!("{FILE_NAME}.1"))).unwrap();
        assert!(previous.contains("line 00"), "{previous}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_panic_line_never_waits_on_the_lock() {
        let held = SINK.lock().unwrap_or_else(PoisonError::into_inner);
        panic_line(format_args!("talaria-desktop: panicked at a test"));
        drop(held);
    }

    #[test]
    fn a_file_already_full_at_launch_starts_over() {
        let dir = scratch("full");
        fs::write(dir.join(FILE_NAME), vec![b'x'; 300]).unwrap();
        let mut sink = Sink::open(&dir, 200).unwrap();
        sink.write(at(0), "talaria-desktop: fresh").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(FILE_NAME)).unwrap(),
            "1970-01-01T00:00:00.000Z talaria-desktop: fresh\n"
        );
        assert_eq!(
            fs::read(dir.join(format!("{FILE_NAME}.1"))).unwrap().len(),
            300
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
