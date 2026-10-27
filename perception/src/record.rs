//! Saves frames as PNG files for the table evaluation set (#8). Only the explicitly enabled
//! debug recording uses this (the dev build's screenshot button and recording switch);
//! otherwise captured frames never touch the disk. Encoding and writing run on their own
//! thread, off the capture → recognition → advice path.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use image::RgbImage;

/// Frames waiting to be written. Further frames are dropped rather than piling up: a
/// 3024×1900 frame is 17 MB.
const QUEUE: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("could not create {}: {source}", path.display())]
    Dir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not start the recorder thread: {0}")]
    Thread(std::io::Error),
}

/// Writes frames into one directory on a background thread; dropping it finishes the
/// queued frames first.
pub struct Recorder {
    dir: PathBuf,
    sender: Option<SyncSender<(PathBuf, RgbImage)>>,
    thread: Option<JoinHandle<()>>,
    /// Frames [`Recorder::save`] dropped for a full queue.
    dropped: Arc<AtomicU64>,
}

impl Recorder {
    /// Starts writing into `dir`, creating it if needed. `on_saved` runs on the recorder
    /// thread after each file is written, with its path and how many have been written.
    pub fn start(
        dir: impl Into<PathBuf>,
        on_saved: impl FnMut(&Path, u64) + Send + 'static,
    ) -> Result<Self, RecordError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|source| RecordError::Dir {
            path: dir.clone(),
            source,
        })?;
        let (sender, frames) = mpsc::sync_channel::<(PathBuf, RgbImage)>(QUEUE);
        let mut on_saved = on_saved;
        let thread = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || {
                let mut written = 0;
                for (path, frame) in frames {
                    match frame.save(&path) {
                        Ok(()) => {
                            written += 1;
                            on_saved(&path, written);
                        }
                        Err(err) => tracing::warn!("saving {}: {err}", path.display()),
                    }
                }
            })
            .map_err(RecordError::Thread)?;
        Ok(Self {
            dir,
            sender: Some(sender),
            thread: Some(thread),
            dropped: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// How many frames [`Recorder::save`] has dropped for a full queue.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn path(&self, label: &str) -> PathBuf {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        self.dir.join(format!("{ms}-{label}.png"))
    }

    /// Queues `frame` to be saved as `<unix ms>-<label>.png`. Returns false, dropping the
    /// frame and counting it, when the queue is full: for the continuous recording, which
    /// must never hold up the watcher.
    pub fn save(&self, label: &str, frame: RgbImage) -> bool {
        let Some(sender) = &self.sender else {
            return false;
        };
        let queued = sender.try_send((self.path(label), frame)).is_ok();
        if !queued {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        queued
    }

    /// Queues `frame` like [`Recorder::save`], but waits for room instead of dropping it:
    /// for a screenshot the player asked for. Returns the file it will be written to.
    /// Blocks while earlier frames are written; call it off the UI thread.
    pub fn save_waiting(&self, label: &str, frame: RgbImage) -> Option<PathBuf> {
        let sender = self.sender.as_ref()?;
        let path = self.path(label);
        sender.send((path.clone(), frame)).ok()?;
        Some(path)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // Closing the channel lets the thread write what is queued and end.
        self.sender.take();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("the recorder thread panicked");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn writes_the_frame_as_png() {
        let dir = std::env::temp_dir().join(format!("mahjong-record-test-{}", std::process::id()));
        let saved = Arc::new(Mutex::new(Vec::new()));
        let recorder = {
            let saved = Arc::clone(&saved);
            Recorder::start(&dir, move |path, count| {
                saved.lock().unwrap().push((path.to_owned(), count));
            })
            .unwrap()
        };
        let frame = RgbImage::from_fn(3, 2, |x, y| image::Rgb([x as u8 * 80, y as u8 * 120, 7]));
        assert!(recorder.save("manual", frame.clone()));
        drop(recorder);

        let saved = saved.lock().unwrap().clone();
        assert_eq!(saved.len(), 1);
        let (path, count) = &saved[0];
        assert_eq!(*count, 1);
        assert!(path.to_string_lossy().ends_with("-manual.png"));
        assert_eq!(image::open(path).unwrap().to_rgb8(), frame);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_full_queue_drops_recorded_frames_but_not_a_screenshot() {
        let dir = std::env::temp_dir().join(format!("mahjong-record-full-{}", std::process::id()));
        // The writer stops after the first frame until told to go on, so the queue fills.
        let (go, wait) = mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let recorder = Recorder::start(&dir, move |_, count| {
            if count == 1 {
                let _ = wait.lock().unwrap().recv();
            }
        })
        .unwrap();
        let frame = RgbImage::new(2, 2);
        assert!(recorder.save("auto", frame.clone()));
        // The first is being written; QUEUE more fill the queue, the next is dropped.
        let mut queued = 0;
        while recorder.save("auto", frame.clone()) {
            queued += 1;
            assert!(queued <= QUEUE + 1, "the queue never filled");
        }
        assert_eq!(recorder.dropped(), 1);
        // A screenshot waits for room instead.
        let shot = std::thread::scope(|s| {
            let waiting = s.spawn(|| recorder.save_waiting("manual", frame.clone()));
            std::thread::sleep(std::time::Duration::from_millis(50));
            go.send(()).unwrap();
            waiting.join().unwrap()
        })
        .unwrap();
        assert!(shot.to_string_lossy().ends_with("-manual.png"));
        drop(recorder);
        assert!(shot.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
