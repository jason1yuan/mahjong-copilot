//! Downloads the models the app needs on first launch, and checks them. This is the only
//! code in the project that goes online, and it runs only when a model is missing.
//!
//! Each model is pinned to a URL (with the revision in it), a size and a SHA-256. A
//! download goes to `<name>.part` next to its final place, resumes from there when it was
//! interrupted, and is moved into place only after its size and hash match, so a file with
//! the final name is always complete and verified.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// A pinned model file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile<'a> {
    /// File name in the models directory; carries the revision, so a new one never mixes
    /// with an old download.
    pub name: &'a str,
    pub url: &'a str,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: &'a str,
}

/// The tile classifier (#4): pjura/mahjong_soul_vision converted to ONNX, hosted by us
/// because the original repository only has PyTorch weights.
pub const TILE_CLASSIFIER: ModelFile<'static> = ModelFile {
    name: "mahjong_soul_vision-d1da028b.onnx",
    url: "https://huggingface.co/xzhe121/mahjong_soul_vision-onnx/resolve/d1da028b53b97e309e67ddfed6e4b61e85603f65/model.onnx",
    size: 344_641_310,
    sha256: "07fd9d39409ff453d6ad06bc83be3f74aa359b24365f5cd70c839237c0fab239",
};

/// The decision model (#22): haor/Mortal-S42 converted to ONNX by
/// `ml/convert/export_mortal.py`, hosted by us for the same reason.
pub const DECISION_MODEL: ModelFile<'static> = ModelFile {
    name: "mortal-s42-2f7ad48b.onnx",
    url: "https://huggingface.co/xzhe121/Mortal-S42-onnx/resolve/2f7ad48bc29dadd9805b1897f3c5b97b76c0d41a/model.onnx",
    size: 43_673_425,
    sha256: "30381bf21389e2d25d3edf48fed11bf772336ef3daeee67d1bdae7fb9dd30cb4",
};

/// Every model the app needs.
pub const ALL: [ModelFile<'static>; 2] = [TILE_CLASSIFIER, DECISION_MODEL];

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("downloading {url}: {source}")]
    Http {
        url: String,
        #[source]
        source: Box<ureq::Error>,
    },
    #[error("{name}: got {got} bytes, expected {expected}")]
    Size {
        name: String,
        got: u64,
        expected: u64,
    },
    #[error("{name}: SHA-256 is {got}, expected {expected}")]
    Hash {
        name: String,
        got: String,
        expected: String,
    },
}

/// How far a download is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
}

/// Bytes between progress reports.
const REPORT_EVERY: u64 = 1 << 20;

/// Where a model lives in `dir`.
pub fn path(dir: &Path, model: &ModelFile) -> PathBuf {
    dir.join(model.name)
}

/// Whether the model is in place. A file only gets its final name after it was verified,
/// so checking the size is enough; [`verify`] rehashes it when that is in doubt.
pub fn is_ready(dir: &Path, model: &ModelFile) -> bool {
    fs::metadata(path(dir, model)).is_ok_and(|m| m.is_file() && m.len() == model.size)
}

/// Rehashes a model file.
pub fn verify(file: &Path, model: &ModelFile) -> Result<(), ModelError> {
    let size = fs::metadata(file)?.len();
    if size != model.size {
        return Err(ModelError::Size {
            name: model.name.to_owned(),
            got: size,
            expected: model.size,
        });
    }
    let got = sha256(file)?;
    if got != model.sha256 {
        return Err(ModelError::Hash {
            name: model.name.to_owned(),
            got,
            expected: model.sha256.to_owned(),
        });
    }
    Ok(())
}

/// Makes sure the model is in `dir`, downloading or resuming it when it is not, and
/// returns its path. Blocks; run it off the UI thread. `progress` is called about every
/// megabyte and once at the end.
pub fn fetch(
    dir: &Path,
    model: &ModelFile,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, ModelError> {
    let target = path(dir, model);
    if is_ready(dir, model) {
        return Ok(target);
    }
    fs::create_dir_all(dir)?;
    let part = dir.join(format!("{}.part", model.name));
    let have = fs::metadata(&part).map_or(0, |m| m.len());
    if have != model.size {
        // Resume a shorter partial file; one that is too long cannot be right.
        let offset = if have < model.size { have } else { 0 };
        download(&part, offset, model, progress)?;
    }
    if let Err(err) = verify(&part, model) {
        // A corrupt partial file would fail the same way next time; start over then.
        fs::remove_file(&part)?;
        return Err(err);
    }
    fs::rename(&part, &target)?;
    tracing::info!(model = model.name, "model downloaded and verified");
    Ok(target)
}

/// Appends the rest of the file from `offset` to `part`, or rewrites it when the server
/// does not resume.
fn download(
    part: &Path,
    offset: u64,
    model: &ModelFile,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), ModelError> {
    let http = |source| ModelError::Http {
        url: model.url.to_owned(),
        source: Box::new(source),
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut request = agent.get(model.url);
    if offset > 0 {
        request = request.header("Range", format!("bytes={offset}-"));
    }
    let response = request.call().map_err(http)?;
    let resumed = offset > 0 && response.status().as_u16() == 206;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(part)?;
    let mut downloaded = if resumed {
        file.seek(SeekFrom::Start(offset))?;
        offset
    } else {
        file.set_len(0)?;
        0
    };
    tracing::info!(model = model.name, from = downloaded, "downloading model");

    let mut reader = response.into_body().into_reader();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut reported = downloaded;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        downloaded += read as u64;
        if downloaded - reported >= REPORT_EVERY {
            progress(Progress {
                downloaded,
                total: model.size,
            });
            reported = downloaded;
        }
    }
    file.sync_all()?;
    progress(Progress {
        downloaded,
        total: model.size,
    });
    Ok(())
}

fn sha256(file: &Path) -> Result<String, io::Error> {
    let mut hasher = Sha256::new();
    let mut file = File::open(file)?;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// How the test server answers.
    #[derive(Clone, Copy)]
    enum Mode {
        /// Honours `Range` with 206.
        Resume,
        /// Ignores `Range` and sends the whole file with 200.
        Whole,
        /// Sends only the first `n` bytes of what was asked for, then hangs up.
        Cut(usize),
    }

    /// A one-request-per-connection HTTP server for `body`; returns its URL and the
    /// `Range` headers it received (empty string for none).
    fn serve(body: Vec<u8>, modes: Vec<Mode>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&ranges);
        std::thread::spawn(move || {
            for (stream, mode) in listener.incoming().zip(modes) {
                let mut stream = stream.unwrap();
                let mut reader = io::BufReader::new(stream.try_clone().unwrap());
                let mut range = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = value.trim().trim_end_matches('-').to_owned();
                    }
                }
                seen.lock().unwrap().push(range.clone());
                let start = match mode {
                    Mode::Whole => 0,
                    _ => range.parse().unwrap_or(0),
                };
                let rest = &body[start..];
                let (status, extra) = if start > 0 {
                    (
                        "206 Partial Content",
                        format!(
                            "Content-Range: bytes {start}-{}/{}\r\n",
                            body.len() - 1,
                            body.len()
                        ),
                    )
                } else {
                    ("200 OK", String::new())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                    rest.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                let sent = match mode {
                    Mode::Cut(n) => &rest[..n.min(rest.len())],
                    _ => rest,
                };
                let _ = stream.write_all(sent);
            }
        });
        (url, ranges)
    }

    fn body() -> Vec<u8> {
        (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn hash(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mahjong-models-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn model<'a>(url: &'a str, sha256: &'a str, size: u64) -> ModelFile<'a> {
        ModelFile {
            name: "test.onnx",
            url,
            size,
            sha256,
        }
    }

    #[test]
    fn downloads_verifies_and_reports_progress() {
        let body = body();
        let (url, ranges) = serve(body.clone(), vec![Mode::Resume]);
        let sha = hash(&body);
        let m = model(&url, &sha, body.len() as u64);
        let dir = temp_dir("fresh");
        let mut reports = Vec::new();
        let file = fetch(&dir, &m, &mut |p| reports.push(p)).unwrap();
        assert_eq!(fs::read(&file).unwrap(), body);
        assert!(is_ready(&dir, &m));
        assert!(reports.len() >= 3, "about every megabyte");
        assert_eq!(reports.last().unwrap().downloaded, body.len() as u64);
        assert_eq!(*ranges.lock().unwrap(), [""]);
        // Already there: no second request (the server would not answer one).
        assert_eq!(fetch(&dir, &m, &mut |_| {}).unwrap(), file);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resumes_an_interrupted_download() {
        let body = body();
        let (url, ranges) = serve(body.clone(), vec![Mode::Cut(1_200_000), Mode::Resume]);
        let sha = hash(&body);
        let m = model(&url, &sha, body.len() as u64);
        let dir = temp_dir("resume");
        assert!(fetch(&dir, &m, &mut |_| {}).is_err(), "cut short");
        assert!(!is_ready(&dir, &m));
        assert_eq!(
            fs::metadata(dir.join("test.onnx.part")).unwrap().len(),
            1_200_000
        );
        let file = fetch(&dir, &m, &mut |_| {}).unwrap();
        assert_eq!(fs::read(file).unwrap(), body);
        assert_eq!(*ranges.lock().unwrap(), ["", "1200000"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn starts_over_when_the_server_does_not_resume() {
        let body = body();
        let (url, ranges) = serve(body.clone(), vec![Mode::Whole]);
        let sha = hash(&body);
        let m = model(&url, &sha, body.len() as u64);
        let dir = temp_dir("whole");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("test.onnx.part"), &body[..1000]).unwrap();
        let file = fetch(&dir, &m, &mut |_| {}).unwrap();
        assert_eq!(fs::read(file).unwrap(), body);
        assert_eq!(*ranges.lock().unwrap(), ["1000"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_a_file_with_the_wrong_hash() {
        let body = body();
        let (url, _) = serve(body.clone(), vec![Mode::Resume]);
        let wrong = hash(b"something else");
        let m = model(&url, &wrong, body.len() as u64);
        let dir = temp_dir("hash");
        let err = fetch(&dir, &m, &mut |_| {}).unwrap_err();
        assert!(matches!(err, ModelError::Hash { .. }), "{err}");
        assert!(!dir.join("test.onnx").exists());
        assert!(
            !dir.join("test.onnx.part").exists(),
            "a bad file is not resumed"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_models_are_pinned_to_a_revision() {
        assert!(
            TILE_CLASSIFIER
                .url
                .contains("/resolve/d1da028b53b97e309e67ddfed6e4b61e85603f65/")
        );
        assert!(
            DECISION_MODEL
                .url
                .contains("/resolve/2f7ad48bc29dadd9805b1897f3c5b97b76c0d41a/")
        );
        for model in ALL {
            assert_eq!(model.sha256.len(), 64);
            assert!(model.url.starts_with("https://huggingface.co/"));
        }
        assert!(ALL.contains(&TILE_CLASSIFIER) && ALL.contains(&DECISION_MODEL));
        assert_ne!(TILE_CLASSIFIER.name, DECISION_MODEL.name);
    }

    /// Downloads the real models (about 390 MB):
    ///     cargo test -p mahjong-models --release -- --ignored
    #[test]
    #[ignore = "downloads about 390 MB from Hugging Face"]
    fn downloads_the_real_models() {
        let dir = temp_dir("real");
        for model in ALL {
            let file = fetch(&dir, &model, &mut |_| {}).unwrap();
            verify(&file, &model).unwrap();
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
