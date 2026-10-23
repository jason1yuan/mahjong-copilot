//! Reads the same picture with Vision in a loop, several ways each round, to see which
//! still work once recognition starts failing (#49).
//!
//! `cargo run -p mahjong-perception --example text_probe -- <png> [seconds between rounds] [accurate|helper]`
//!
//! `helper` reads through the app's helper process instead (this example started with
//! `--text-helper`); kill that process to see it replaced.

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Cursor;
    use std::time::{Duration, Instant};

    use image::ImageFormat;
    use objc2::AllocAnyThread;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
    };

    #[derive(Clone, Copy, Debug)]
    enum Way {
        Accurate,
        CpuOnly,
        Fast,
        Revision2,
    }

    fn read(png: &[u8], way: Way) -> Result<usize, String> {
        autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(match way {
                Way::Fast => VNRequestTextRecognitionLevel::Fast,
                _ => VNRequestTextRecognitionLevel::Accurate,
            });
            if matches!(way, Way::CpuOnly) {
                #[allow(deprecated)]
                // SAFETY: a plain flag on a request this thread made.
                unsafe {
                    request.setUsesCPUOnly(true)
                };
            }
            if matches!(way, Way::Revision2) {
                // SAFETY: a plain number on a request this thread made.
                unsafe { request.setRevision(2) };
            }
            let languages: Vec<Retained<NSString>> = ["zh-Hant", "ja-JP"]
                .iter()
                .map(|l| NSString::from_str(l))
                .collect();
            request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));
            request.setUsesLanguageCorrection(false);
            let handler = VNImageRequestHandler::initWithData_options(
                VNImageRequestHandler::alloc(),
                &NSData::with_bytes(png),
                &NSDictionary::new(),
            );
            let as_request: &VNRequest = &request;
            handler
                .performRequests_error(&NSArray::from_slice(&[as_request]))
                .map_err(|e| {
                    format!(
                        "{} (domain {}, code {})",
                        e.localizedDescription(),
                        e.domain(),
                        e.code()
                    )
                })?;
            Ok(request.results().map_or(0, |r| r.len()))
        })
    }

    use mahjong_perception::text::{self, ReadText};

    if std::env::args().nth(1).as_deref() == Some(text::HELPER_ARG) {
        return Ok(text::serve()?);
    }
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: text_probe <png> [seconds]")?;
    let every = Duration::from_secs_f64(args.next().map_or(Ok(5.0), |s| s.parse())?);
    let frame = image::open(&path)?.to_rgb8();
    let (w, h) = frame.dimensions();
    // The centre of the table: round label and scores.
    let crop = image::imageops::crop_imm(&frame, w * 2 / 5, h * 2 / 5, w / 5, h / 5).to_image();
    let mut png = Vec::new();
    crop.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    let mode = args.next();
    if mode.as_deref() == Some("helper") {
        let mut helper = text::Helper::new(std::env::current_exe()?);
        loop {
            let start = Instant::now();
            let result = helper.read_text(&crop);
            println!(
                "{:?} | Helper {:?} {:?} failing={}",
                std::time::SystemTime::now(),
                result.map(|lines| lines.len()),
                start.elapsed(),
                helper.failing()
            );
            std::thread::sleep(every);
        }
    }
    let all = mode.is_none();
    let ways: &[Way] = if all {
        &[Way::Accurate, Way::CpuOnly, Way::Fast, Way::Revision2]
    } else {
        &[Way::Accurate]
    };
    loop {
        let mut line = format!("{:?}", std::time::SystemTime::now());
        for &way in ways {
            let start = Instant::now();
            let result = read(&png, way);
            line += &format!(" | {way:?} {result:?} {:?}", start.elapsed());
        }
        if all {
            let png2 = png.clone();
            let start = Instant::now();
            let fresh = std::thread::spawn(move || read(&png2, Way::Accurate))
                .join()
                .map_err(|_| "probe thread panicked")?;
            line += &format!(" | NewThread {fresh:?} {:?}", start.elapsed());
        }
        println!("{line}");
        std::thread::sleep(every);
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
