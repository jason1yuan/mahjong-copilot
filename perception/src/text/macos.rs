//! Text recognition with Vision's `VNRecognizeTextRequest`, which runs on the device.

use std::io::Cursor;

use image::{ImageFormat, RgbImage};
use objc2::AllocAnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

use super::{ReadText, TextError};

/// Languages to recognize, most likely first: the game shows traditional Chinese or
/// Japanese characters (東, 局).
const LANGUAGES: [&str; 2] = ["zh-Hant", "ja-JP"];

/// Holds nothing: Vision objects are made per call, on the calling thread, and the system
/// keeps the loaded model between calls.
pub(super) struct VisionText;

impl ReadText for VisionText {
    fn read_text(&mut self, image: &RgbImage) -> Result<Vec<String>, TextError> {
        // A small PNG in memory; Vision decodes it itself, which spares building a CGImage.
        let mut png = Vec::new();
        image.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
        autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            let languages: Vec<Retained<NSString>> =
                LANGUAGES.iter().map(|l| NSString::from_str(l)).collect();
            request.setRecognitionLanguages(&NSArray::from_retained_slice(&languages));
            // Labels are not words: correction only turns digits into look-alike letters.
            request.setUsesLanguageCorrection(false);
            let handler = VNImageRequestHandler::initWithData_options(
                VNImageRequestHandler::alloc(),
                &NSData::with_bytes(&png),
                &NSDictionary::new(),
            );
            let as_request: &VNRequest = &request;
            handler
                .performRequests_error(&NSArray::from_slice(&[as_request]))
                .map_err(|err| TextError::Recognize(err.localizedDescription().to_string()))?;
            Ok(request
                .results()
                .map(|observations| {
                    observations
                        .iter()
                        .filter_map(|o| o.topCandidates(1).firstObject())
                        .map(|text| text.string().to_string())
                        .collect()
                })
                .unwrap_or_default())
        })
    }
}
