//! Saves frames as PNG files for the table evaluation set (#8). Only the explicitly enabled
//! debug recording uses this (the dev build's screenshot button and recording switch);
//! otherwise captured frames never touch the disk. Encoding and writing run on their own
//! thread, off the capture → recognition → advice path.

use std::path::{Path, PathBuf};
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
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Queues `frame` to be saved as `<unix ms>-<label>.png`. Returns false, dropping the
    /// frame, when the queue is full.
    pub fn save(&self, label: &str, frame: RgbImage) -> bool {
        let Some(sender) = &self.sender else {
            return false;
        };
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let path = self.dir.join(format!("{ms}-{label}.png"));
        sender.try_send((path, frame)).is_ok()
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
}
