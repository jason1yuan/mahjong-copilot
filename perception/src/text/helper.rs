//! Text recognition in a child process (#49). On macOS 27, Vision can start failing every
//! request with `CRImageReaderError error 1` and stay that way for the rest of the
//! process, while a fresh process reads the same picture fine. So the app reads text in a
//! helper, its own executable started with [`HELPER_ARG`], and replaces the helper when it
//! fails.
//!
//! The two talk over the helper's stdin and stdout: a picture goes in as its width, height
//! and RGB bytes; the lines come back as JSON. Nothing is written to disk and nothing leaves
//! the machine.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use image::RgbImage;

use super::{ReadText, TextError};

/// The argument that makes the app's executable serve text recognition instead of
/// starting the app; see [`serve`].
pub const HELPER_ARG: &str = "--text-helper";

/// While even a fresh helper fails, how long to wait before starting another: loading
/// Vision costs a fresh process about 150 ms, and failing readings are skipped meanwhile.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// Largest picture the helper takes, in pixels a side: labels are a few hundred.
const MAX_SIDE: u32 = 4096;

/// Largest answer, in bytes.
const MAX_ANSWER: u32 = 1 << 20;

type Answer = Result<Vec<String>, String>;

fn write_request(to: &mut impl Write, image: &RgbImage) -> io::Result<()> {
    to.write_all(&image.width().to_le_bytes())?;
    to.write_all(&image.height().to_le_bytes())?;
    to.write_all(image.as_raw())?;
    to.flush()
}

/// The next picture; `None` at the end of the input.
fn read_request(from: &mut impl Read) -> io::Result<Option<RgbImage>> {
    let mut size = [0; 8];
    match from.read_exact(&mut size[..4]) {
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        other => other?,
    }
    from.read_exact(&mut size[4..])?;
    let [w0, w1, w2, w3, h0, h1, h2, h3] = size;
    let (width, height) = (
        u32::from_le_bytes([w0, w1, w2, w3]),
        u32::from_le_bytes([h0, h1, h2, h3]),
    );
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a {width}x{height} picture is too large"),
        ));
    }
    let mut pixels = vec![0; width as usize * height as usize * 3];
    from.read_exact(&mut pixels)?;
    RgbImage::from_raw(width, height, pixels)
        .map(Some)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "short picture"))
}

fn write_answer(to: &mut impl Write, answer: &Answer) -> io::Result<()> {
    let json = serde_json::to_vec(answer)?;
    let len = u32::try_from(json.len())
        .ok()
        .filter(|&len| len <= MAX_ANSWER)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "answer too long"))?;
    to.write_all(&len.to_le_bytes())?;
    to.write_all(&json)?;
    to.flush()
}

fn read_answer(from: &mut impl Read) -> io::Result<Answer> {
    let mut len = [0; 4];
    from.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_ANSWER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "answer too long",
        ));
    }
    let mut json = vec![0; len as usize];
    from.read_exact(&mut json)?;
    Ok(serde_json::from_slice(&json)?)
}

/// Answers pictures from `input` with `text` on `output` until `input` ends.
fn serve_on(text: &mut dyn ReadText, input: impl Read, output: impl Write) -> io::Result<()> {
    let (mut input, mut output) = (BufReader::new(input), BufWriter::new(output));
    while let Some(image) = read_request(&mut input)? {
        let answer = text.read_text(&image).map_err(|err| err.to_string());
        write_answer(&mut output, &answer)?;
    }
    Ok(())
}

/// The helper's whole life: answers pictures on stdin with the system's text recognition
/// until stdin closes, which it does when the app quits or replaces it.
pub fn serve() -> io::Result<()> {
    let Some(mut text) = super::system() else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no text recognition on this platform",
        ));
    };
    serve_on(text.as_mut(), io::stdin().lock(), io::stdout().lock())
}

/// One running helper, or a stand-in for it in tests.
trait Connection: Send {
    fn ask(&mut self, image: &RgbImage) -> io::Result<Answer>;
}

struct Process {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl Process {
    fn start(exe: &PathBuf) -> io::Result<Self> {
        let mut child = Command::new(exe)
            .arg(HELPER_ARG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Its warnings go to the app's log.
            .stderr(Stdio::inherit())
            .spawn()?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("the helper's pipes are missing"));
        };
        Ok(Self {
            child,
            input: BufWriter::new(input),
            output: BufReader::new(output),
        })
    }
}

impl Connection for Process {
    fn ask(&mut self, image: &RgbImage) -> io::Result<Answer> {
        write_request(&mut self.input, image)?;
        read_answer(&mut self.output)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Closing stdin would end it too; killing does not wait for a stuck request.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

type Start = dyn FnMut() -> io::Result<Box<dyn Connection>> + Send;

struct Inner {
    start: Box<Start>,
    connection: Option<Box<dyn Connection>>,
    /// Even a fresh helper failed: readings are skipped until `retry_at`.
    retry_at: Option<Instant>,
    /// Readings failed since the last one that worked.
    failed: usize,
}

impl Inner {
    /// One try with the running helper, started if there is none. A failure drops it.
    fn try_once(&mut self, image: &RgbImage) -> Result<Vec<String>, TextError> {
        let connection = match &mut self.connection {
            Some(connection) => connection,
            None => self
                .connection
                .insert((self.start)().map_err(|err| TextError::Helper(err.to_string()))?),
        };
        let answer = connection
            .ask(image)
            .map_err(|err| TextError::Helper(err.to_string()))
            .and_then(|answer| answer.map_err(TextError::Recognize));
        if answer.is_err() {
            self.connection = None;
        }
        answer
    }

    fn read(&mut self, image: &RgbImage) -> Result<Vec<String>, TextError> {
        if let Some(at) = self.retry_at
            && Instant::now() < at
        {
            self.failed += 1;
            return Err(TextError::Unavailable);
        }
        let fresh = self.connection.is_none();
        let mut answer = self.try_once(image);
        if let Err(err) = &answer
            && !fresh
        {
            // The helper that was reading fine fails: try the picture again in a new one.
            tracing::debug!("text recognition failed; starting a new helper: {err}");
            answer = self.try_once(image);
        }
        match &answer {
            Ok(_) => {
                if self.failed > 0 {
                    tracing::info!(
                        failed = self.failed,
                        "text recognition works again in a new helper"
                    );
                }
                self.failed = 0;
                self.retry_at = None;
            }
            Err(err) => {
                if self.failed == 0 {
                    tracing::warn!(
                        "text recognition fails even in a new helper; retrying every {}s: {err}",
                        RETRY_AFTER.as_secs()
                    );
                } else {
                    tracing::warn!(
                        failed = self.failed + 1,
                        "text recognition still fails: {err}"
                    );
                }
                self.failed += 1;
                self.retry_at = Some(Instant::now() + RETRY_AFTER);
            }
        }
        answer
    }
}

/// Text recognition in a helper process, replaced when it fails. Clones share the helper.
#[derive(Clone)]
pub struct Helper {
    inner: Arc<Mutex<Inner>>,
    failing: Arc<AtomicBool>,
}

// Holds only counters and the helper, which a panic elsewhere cannot leave half-written.
fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Helper {
    /// Reads with `exe` started with [`HELPER_ARG`], which must call [`serve`]. The helper
    /// starts on the first reading.
    pub fn new(exe: PathBuf) -> Self {
        Self::with(Box::new(move || {
            Process::start(&exe).map(|p| Box::new(p) as Box<dyn Connection>)
        }))
    }

    fn with(start: Box<Start>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                start,
                connection: None,
                retry_at: None,
                failed: 0,
            })),
            failing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether text cannot be read even in a fresh helper; it is tried again now and then.
    pub fn failing(&self) -> bool {
        self.failing.load(Ordering::Relaxed)
    }

    /// Starts the helper and has it load the system's text recognition, so the first real
    /// reading does not wait for that. Meant for a background thread.
    pub fn warm_up(&self) {
        let _ = lock(&self.inner).try_once(&RgbImage::new(64, 32));
    }
}

impl ReadText for Helper {
    fn read_text(&mut self, image: &RgbImage) -> Result<Vec<String>, TextError> {
        let answer = lock(&self.inner).read(image);
        self.failing.store(answer.is_err(), Ordering::Relaxed);
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Echo;

    impl ReadText for Echo {
        fn read_text(&mut self, image: &RgbImage) -> Result<Vec<String>, TextError> {
            if image.width() == 1 {
                return Err(TextError::Recognize("too small".into()));
            }
            Ok(vec![
                format!("{}x{}", image.width(), image.height()),
                "東1局".into(),
            ])
        }
    }

    #[test]
    fn serves_pictures_until_the_input_ends() {
        let mut input = Vec::new();
        write_request(&mut input, &RgbImage::new(3, 2)).unwrap();
        write_request(&mut input, &RgbImage::new(1, 1)).unwrap();
        let mut output = Vec::new();
        serve_on(&mut Echo, input.as_slice(), &mut output).unwrap();
        let mut answers = output.as_slice();
        assert_eq!(
            read_answer(&mut answers).unwrap(),
            Ok(vec!["3x2".to_owned(), "東1局".to_owned()])
        );
        assert_eq!(
            read_answer(&mut answers).unwrap(),
            Err("text recognition failed: too small".to_owned())
        );
        assert!(answers.is_empty());
    }

    #[test]
    fn refuses_a_picture_too_large() {
        let mut input = Vec::new();
        input.extend_from_slice(&(MAX_SIDE + 1).to_le_bytes());
        input.extend_from_slice(&1u32.to_le_bytes());
        assert!(serve_on(&mut Echo, input.as_slice(), Vec::new()).is_err());
    }

    /// A helper that answers from a script, then fails.
    struct Scripted(VecDeque<io::Result<Answer>>);

    impl Connection for Scripted {
        fn ask(&mut self, _: &RgbImage) -> io::Result<Answer> {
            self.0
                .pop_front()
                .unwrap_or_else(|| Err(io::Error::other("closed")))
        }
    }

    /// A helper that starts each helper from the next script, and counts the starts.
    fn scripted(scripts: Vec<Vec<io::Result<Answer>>>) -> (Helper, Arc<Mutex<usize>>) {
        let started = Arc::new(Mutex::new(0));
        let count = Arc::clone(&started);
        let mut scripts: VecDeque<_> = scripts.into_iter().collect();
        let helper = Helper::with(Box::new(move || {
            *count.lock().unwrap() += 1;
            let script = scripts.pop_front().unwrap_or_default();
            Ok(Box::new(Scripted(script.into())) as Box<dyn Connection>)
        }));
        (helper, started)
    }

    fn lines() -> io::Result<Answer> {
        Ok(Ok(vec!["東1局".to_owned()]))
    }

    fn broken() -> io::Result<Answer> {
        Ok(Err("CRImageReaderError error 1".to_owned()))
    }

    #[test]
    fn replaces_a_helper_that_starts_failing() {
        let (mut helper, started) = scripted(vec![vec![lines(), broken()], vec![lines()]]);
        let image = RgbImage::new(4, 4);
        assert!(helper.read_text(&image).is_ok());
        // The second reading fails in the first helper and works in a new one.
        assert!(helper.read_text(&image).is_ok());
        assert_eq!(*started.lock().unwrap(), 2);
        assert!(!helper.failing());
    }

    #[test]
    fn waits_before_retrying_when_a_fresh_helper_fails_too() {
        let (mut helper, started) = scripted(vec![
            vec![lines(), broken()],
            vec![broken()],
            vec![broken()],
            vec![lines()],
        ]);
        let image = RgbImage::new(4, 4);
        assert!(helper.read_text(&image).is_ok());
        assert!(matches!(
            helper.read_text(&image),
            Err(TextError::Recognize(_))
        ));
        assert!(helper.failing());
        // No new helper until the wait is over.
        assert!(matches!(
            helper.read_text(&image),
            Err(TextError::Unavailable)
        ));
        assert_eq!(*started.lock().unwrap(), 2);
        lock(&helper.inner).retry_at = Some(Instant::now());
        assert!(helper.read_text(&image).is_err());
        assert_eq!(*started.lock().unwrap(), 3);
        assert!(helper.failing());
        // The next helper works: readings come back.
        lock(&helper.inner).retry_at = Some(Instant::now());
        assert!(helper.read_text(&image).is_ok());
        assert_eq!(*started.lock().unwrap(), 4);
        assert!(!helper.failing());
    }

    #[test]
    fn a_helper_that_dies_is_replaced() {
        let (mut helper, started) = scripted(vec![vec![lines()], vec![lines()]]);
        let image = RgbImage::new(4, 4);
        assert!(helper.read_text(&image).is_ok());
        // The first helper's script is over: its pipe reads as closed.
        assert!(helper.read_text(&image).is_ok());
        assert_eq!(*started.lock().unwrap(), 2);
    }
}
