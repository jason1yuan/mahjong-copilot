//! The dev app's log file (#80): what the log says also goes to `logs/<UTC date>.log` in
//! the app's data folder, so a packaged dev app leaves a record to debug from. Only the
//! dev tools turn it on; the release app logs to stderr alone. Lines go to a writer thread
//! through a bounded queue and are dropped when it is full, so logging never blocks the
//! capture → recognition → advice path. Frames are never logged.

use std::fs::{File, OpenOptions};
use std::io::{self, LineWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing_subscriber::fmt::MakeWriter;

/// Lines waiting to be written.
const QUEUE: usize = 1024;
/// Log files older than this are deleted when the app starts.
const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

static LINES: OnceLock<SyncSender<Vec<u8>>> = OnceLock::new();

/// Starts writing the log into `dir`, removing files older than a week. Returns the file.
pub fn start(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    remove_old(dir);
    let path = dir.join(format!("{}.log", utc_date(SystemTime::now())));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let (sender, lines) = mpsc::sync_channel::<Vec<u8>>(QUEUE);
    std::thread::Builder::new()
        .name("dev-log".into())
        .spawn(move || write_lines(LineWriter::new(file), lines))?;
    // Started once; a second start keeps the first writer.
    let _ = LINES.set(sender);
    Ok(path)
}

fn write_lines(mut file: LineWriter<File>, lines: mpsc::Receiver<Vec<u8>>) {
    for line in lines {
        if file.write_all(&line).is_err() {
            // Nowhere left to report it: the log is what failed.
            return;
        }
    }
}

fn remove_old(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > KEEP);
        if old && path.extension().is_some_and(|e| e == "log") {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `YYYY-MM-DD` in UTC.
fn utc_date(time: SystemTime) -> String {
    let days = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The log layer's writer: each formatted event goes to the writer thread once
/// [`start`] ran, and nowhere before.
#[derive(Clone, Copy)]
pub struct DevLog;

pub struct Line;

impl Write for Line {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(lines) = LINES.get() {
            // Full: dropped, as logging must not block.
            let _ = lines.try_send(buf.to_vec());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for DevLog {
    type Writer = Line;

    fn make_writer(&'a self) -> Self::Writer {
        Line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc_calendar_days() {
        assert_eq!(utc_date(UNIX_EPOCH), "1970-01-01");
        // 2026-10-26 00:00 and 23:59 UTC.
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(1_792_972_800)),
            "2026-10-26"
        );
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(1_792_972_800 + 86_399)),
            "2026-10-26"
        );
        // A leap day.
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_secs(1_709_164_800)),
            "2024-02-29"
        );
    }
}
