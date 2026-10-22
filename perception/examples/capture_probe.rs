//! Captures one window and reports what the pipeline would see (#1): frames, frames that
//! changed, regions that settled, the frame size and our own CPU time. Nothing is saved.
//!
//! Lists windows without arguments; otherwise picks the first window whose title or app
//! name contains the given text:
//!     cargo run -p mahjong-perception --release --example capture_probe
//!     cargo run -p mahjong-perception --release --example capture_probe -- 雀魂 20

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mahjong_perception::capture::{self, CaptureConfig, LatestFrame, RelRect};
use mahjong_perception::change::{ChangeDetector, FrameGate, Region};

fn cpu_seconds() -> f64 {
    // SAFETY: getrusage only writes into the zeroed struct we pass.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let t = |v: libc::timeval| v.tv_sec as f64 + f64::from(v.tv_usec) / 1e6;
    t(usage.ru_utime) + t(usage.ru_stime)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(needle) = args.next() else {
        for w in capture::list_windows()? {
            println!(
                "{:>8}  {:4.0}x{:<4.0}  {} — {}",
                w.id, w.width, w.height, w.app, w.title
            );
        }
        return Ok(());
    };
    let seconds: u64 = args.next().map_or(Ok(15), |s| s.parse())?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let window = loop {
        let found = capture::list_windows()?
            .into_iter()
            .find(|w| w.title.contains(&needle) || w.app.contains(&needle));
        match found {
            Some(w) => break w,
            None if Instant::now() > deadline => {
                return Err(format!("no window matching {needle:?}").into());
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    println!(
        "capturing {} — {} ({:.0}x{:.0} pt)",
        window.app, window.title, window.width, window.height
    );

    let latest = Arc::new(LatestFrame::new());
    let passed = Arc::new(AtomicU64::new(0));
    let session = {
        let latest = Arc::clone(&latest);
        let passed = Arc::clone(&passed);
        let mut gate = FrameGate::new();
        capture::start(window.id, CaptureConfig::default(), move |frame| {
            if gate.changed(frame) {
                passed.fetch_add(1, Ordering::Relaxed);
                latest.publish(frame);
            }
        })?
    };

    // The pipeline side: the whole window plus its four quarters as regions.
    let mut detector = ChangeDetector::new(&[
        Region {
            rect: RelRect::FULL,
            cols: 32,
            rows: 18,
        },
        Region {
            rect: RelRect::new(0.0, 0.0, 0.5, 0.5),
            cols: 16,
            rows: 9,
        },
        Region {
            rect: RelRect::new(0.5, 0.0, 0.5, 0.5),
            cols: 16,
            rows: 9,
        },
        Region {
            rect: RelRect::new(0.0, 0.5, 0.5, 0.5),
            cols: 16,
            rows: 9,
        },
        Region {
            rect: RelRect::new(0.5, 0.5, 0.5, 0.5),
            cols: 16,
            rows: 9,
        },
    ]);
    let start = Instant::now();
    let cpu_start = cpu_seconds();
    let mut seen = 0;
    let mut second = 0;
    let mut events = Vec::new();
    let (mut last_frames, mut last_passed, mut last_cpu) = (0, 0, cpu_start);
    let mut size = (0, 0, 0.0);
    while start.elapsed() < Duration::from_secs(seconds) {
        let wait = detector
            .next_check()
            .map_or(Duration::from_millis(200), |t| {
                t.saturating_duration_since(Instant::now())
            })
            .min(Duration::from_millis(200));
        if let Some(frame) = latest.wait_newer(seen, Some(wait)) {
            seen = frame.sequence();
            if let Some(view) = frame.view() {
                detector.observe(&view, Instant::now());
                size = (view.width(), view.height(), view.scale());
            }
        }
        for region in detector.settled(Instant::now()) {
            events.push(format!(
                "{:.2}s settled region {region}",
                start.elapsed().as_secs_f64()
            ));
        }
        if start.elapsed().as_secs() > second {
            let stats = session.stats();
            let cpu = cpu_seconds();
            let now_passed = passed.load(Ordering::Relaxed);
            println!(
                "{second:3}s  frames {:3}  changed {:3}  size {}x{} @{:.2}x  resized {}  cpu {:4.1}%  {}",
                stats.frames - last_frames,
                now_passed - last_passed,
                size.0,
                size.1,
                size.2,
                stats.resized,
                (cpu - last_cpu) * 100.0,
                events.join(", "),
            );
            events.clear();
            (last_frames, last_passed, last_cpu) = (stats.frames, now_passed, cpu);
            second += 1;
        }
    }
    let stats = session.stats();
    session.stop()?;
    println!(
        "total: {} frames, {} changed, {} idle notices, {} resizes, stopped by the system: {}; cpu {:.2} s over {} s",
        stats.frames,
        passed.load(Ordering::Relaxed),
        stats.idle,
        stats.resized,
        stats.stopped,
        cpu_seconds() - cpu_start,
        seconds
    );
    Ok(())
}
