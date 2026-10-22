//! Hands the newest frame from the capture thread to the pipeline without queueing: a
//! slower consumer simply sees the latest frame. The pixel buffer is reused, so after the
//! first frame (and after a resize) publishing a frame does not allocate.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::FrameView;

/// A frame copied out of the capture system, tightly packed BGRA.
#[derive(Debug, Default)]
pub struct OwnedFrame {
    width: u32,
    height: u32,
    scale: f64,
    sequence: u64,
    captured: Option<Instant>,
    bgra: Vec<u8>,
}

impl OwnedFrame {
    /// Counts published frames; 0 until the first one.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn captured(&self) -> Option<Instant> {
        self.captured
    }

    /// `None` until the first frame.
    pub fn view(&self) -> Option<FrameView<'_>> {
        if self.sequence == 0 {
            return None;
        }
        FrameView::new(
            self.width,
            self.height,
            self.width as usize * 4,
            &self.bgra,
            self.scale,
        )
    }
}

#[derive(Default)]
pub struct LatestFrame {
    frame: Mutex<OwnedFrame>,
    fresh: Condvar,
}

impl LatestFrame {
    pub fn new() -> Self {
        Self::default()
    }

    // A panic elsewhere while holding the lock leaves at worst a half-copied frame; the
    // next frame overwrites it, so a poisoned lock is used as is.
    fn lock(&self) -> MutexGuard<'_, OwnedFrame> {
        self.frame.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Copies a frame in, replacing the previous one, and wakes a waiting consumer.
    pub fn publish(&self, frame: &FrameView<'_>) {
        let mut owned = self.lock();
        let row = frame.width() as usize * 4;
        owned.bgra.resize(row * frame.height() as usize, 0);
        for (y, out) in owned.bgra.chunks_exact_mut(row.max(1)).enumerate() {
            let start = y * frame.stride();
            out.copy_from_slice(&frame.bgra()[start..start + row]);
        }
        owned.width = frame.width();
        owned.height = frame.height();
        owned.scale = frame.scale();
        owned.sequence += 1;
        owned.captured = Some(Instant::now());
        drop(owned);
        self.fresh.notify_all();
    }

    /// The latest frame, whatever its age.
    pub fn latest(&self) -> MutexGuard<'_, OwnedFrame> {
        self.lock()
    }

    /// Waits for a frame newer than sequence `seen`, up to `timeout` (forever if `None`).
    /// Returns the frame if one arrived; hold the guard only briefly, it blocks capture.
    pub fn wait_newer(
        &self,
        seen: u64,
        timeout: Option<Duration>,
    ) -> Option<MutexGuard<'_, OwnedFrame>> {
        let guard = self.lock();
        let is_old = |f: &mut OwnedFrame| f.sequence <= seen;
        let guard = match timeout {
            None => self
                .fresh
                .wait_while(guard, is_old)
                .unwrap_or_else(PoisonError::into_inner),
            Some(timeout) => {
                self.fresh
                    .wait_timeout_while(guard, timeout, is_old)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
        };
        (guard.sequence > seen).then_some(guard)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::capture::tests::bgra;

    #[test]
    fn keeps_only_the_newest_frame_and_drops_padding() {
        let latest = LatestFrame::new();
        assert!(latest.latest().view().is_none());
        // 3 pixels wide with 4 pixels of stride: the padding must not be copied.
        let mut data = bgra(4, 2, |x, y| [x as u8, y as u8, 0]);
        data.truncate(4 * 4 + 3 * 4);
        let frame = FrameView::new(3, 2, 16, &data, 2.0).unwrap();
        latest.publish(&frame);
        latest.publish(&frame);
        let owned = latest.latest();
        assert_eq!(owned.sequence(), 2);
        let view = owned.view().unwrap();
        assert_eq!((view.width(), view.height(), view.stride()), (3, 2, 12));
        assert_eq!(view.rgb(2, 1), [2, 1, 0]);
        assert_eq!(view.scale(), 2.0);
    }

    #[test]
    fn a_waiting_consumer_wakes_up_on_a_new_frame() {
        let latest = Arc::new(LatestFrame::new());
        let producer = Arc::clone(&latest);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let data = bgra(2, 2, |_, _| [9, 9, 9]);
            producer.publish(&FrameView::new(2, 2, 8, &data, 1.0).unwrap());
        });
        let frame = latest.wait_newer(0, Some(Duration::from_secs(5))).unwrap();
        assert_eq!(frame.sequence(), 1);
        drop(frame);
        thread.join().unwrap();
        assert!(
            latest
                .wait_newer(1, Some(Duration::from_millis(10)))
                .is_none(),
            "nothing newer"
        );
    }
}
