//! Tile recognition as the pipeline uses it: the template cache first, the classifier for
//! the rest, and a doubt mark on tiles it should not be trusted with.
//!
//! A tile is in doubt when its crop is narrower than [`MIN_TILE_WIDTH`] (the classifier
//! and the red-five rule start failing on tiny tiles; the window should be made larger) or
//! the classifier's confidence is below [`MIN_CONFIDENCE`]. Tiles in doubt keep their best
//! guess for display (with a "?" mark) but never become templates. The thresholds come from
//! the evaluation crops (see the PR for #4): on the multi-source simulations they mark 71 of
//! the 76 classifier errors, and none of the 132 real hand tiles.
//!
//! Each zone of the table (the hand, each seat's river) keeps its own templates, since the
//! same tile looks different in each (size, angle, light); they share the one classifier.
//!
//! A doubtful answer is remembered only for the same picture, so a tile that stays in doubt
//! on the table (under a glare, or too small) is not sent to the classifier on every reading.
//!
//! Every other answer becomes a template. A stricter bar for templates (0.9) was tried and
//! dropped: it cost hits and fixed nothing, since the one wrong hit seen repeated a mistake
//! the classifier made with 0.95 confidence, and wherever a hit differed from what the
//! classifier would have said, the hit was the right one (a blurred crop matching a clear
//! earlier template).

use std::collections::HashMap;

use image::RgbImage;
use mahjong_core::Tile;

use crate::cache::TemplateCache;
use crate::classifier::{ClassifierError, Prediction, TileClassifier};
use crate::table::Seat;

/// Crops narrower than this are in doubt.
pub const MIN_TILE_WIDTH: u32 = 28;
/// Classifier answers less confident than this are in doubt.
pub const MIN_CONFIDENCE: f32 = 0.7;

/// What turns crops into tiles when the cache has no answer.
pub trait Classify {
    /// One prediction per crop, red fives included.
    fn recognize(&mut self, crops: &[RgbImage]) -> Result<Vec<Prediction>, ClassifierError>;
}

impl Classify for TileClassifier {
    fn recognize(&mut self, crops: &[RgbImage]) -> Result<Vec<Prediction>, ClassifierError> {
        TileClassifier::recognize(self, crops)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Cache,
    Classifier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Doubt {
    /// The crop is too small to read reliably; the window should be made larger.
    TooSmall,
    /// The classifier is not confident.
    LowConfidence,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recognition {
    /// Best guess, also when in doubt.
    pub tile: Tile,
    /// Classifier confidence (for a cache hit, the one its template was stored with).
    pub confidence: f32,
    pub source: Source,
    pub doubt: Option<Doubt>,
}

/// Where on the table a tile is, which picks its template cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Zone {
    Hand,
    River(Seat),
    Meld(Seat),
    Dora,
}

/// One per game, tile skin and window size, like its template caches.
pub struct TileRecognizer<C = TileClassifier> {
    classifier: C,
    caches: HashMap<Zone, TemplateCache>,
}

impl<C: Classify> TileRecognizer<C> {
    pub fn new(classifier: C) -> Self {
        Self {
            classifier,
            caches: HashMap::new(),
        }
    }

    /// The templates of `zone`, if it has any yet.
    pub fn cache(&self, zone: Zone) -> Option<&TemplateCache> {
        self.caches.get(&zone)
    }

    /// Forgets all templates; call when the game, skin or window size changes.
    pub fn clear_cache(&mut self) {
        self.caches.clear();
    }

    /// Recognizes hand tiles; see [`recognize_in`](Self::recognize_in).
    pub fn recognize(&mut self, crops: &[RgbImage]) -> Result<Vec<Recognition>, ClassifierError> {
        self.recognize_in(Zone::Hand, crops)
    }

    /// Recognizes tile crops from `zone`: cache hits right away, the rest in one classifier
    /// batch.
    pub fn recognize_in(
        &mut self,
        zone: Zone,
        crops: &[RgbImage],
    ) -> Result<Vec<Recognition>, ClassifierError> {
        let cache = self.caches.entry(zone).or_default();
        let mut results: Vec<Option<Recognition>> = crops
            .iter()
            .map(|crop| {
                let small = crop.width() < MIN_TILE_WIDTH;
                if let Some(hit) = (!small).then(|| cache.lookup(crop)).flatten() {
                    return Some(Recognition {
                        tile: hit.tile,
                        confidence: hit.confidence,
                        source: Source::Cache,
                        doubt: None,
                    });
                }
                // The same doubtful picture as before keeps its answer and its doubt.
                let (tile, confidence) = cache.recall_doubtful(crop)?;
                Some(Recognition {
                    tile,
                    confidence,
                    source: Source::Cache,
                    doubt: Some(if small {
                        Doubt::TooSmall
                    } else {
                        Doubt::LowConfidence
                    }),
                })
            })
            .collect();
        let misses: Vec<usize> = (0..crops.len()).filter(|&i| results[i].is_none()).collect();
        if !misses.is_empty() {
            let batch: Vec<RgbImage> = misses.iter().map(|&i| crops[i].clone()).collect();
            let predictions = self.classifier.recognize(&batch)?;
            for (&i, prediction) in misses.iter().zip(predictions) {
                let doubt = if crops[i].width() < MIN_TILE_WIDTH {
                    Some(Doubt::TooSmall)
                } else if prediction.confidence < MIN_CONFIDENCE {
                    Some(Doubt::LowConfidence)
                } else {
                    None
                };
                if doubt.is_none() {
                    cache.insert(prediction.tile, prediction.confidence, &crops[i]);
                } else {
                    cache.remember_doubtful(prediction.tile, prediction.confidence, &crops[i]);
                }
                results[i] = Some(Recognition {
                    tile: prediction.tile,
                    confidence: prediction.confidence,
                    source: Source::Classifier,
                    doubt,
                });
            }
        }
        // Every crop was either a cache hit or in the classifier batch, which returns one
        // prediction per crop.
        Ok(results.into_iter().flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    /// Says "3m" for light crops and "7p" for dark ones, with a fixed confidence, and counts
    /// the crops it was asked about.
    struct Fake {
        confidence: f32,
        asked: usize,
    }

    impl Classify for Fake {
        fn recognize(&mut self, crops: &[RgbImage]) -> Result<Vec<Prediction>, ClassifierError> {
            self.asked += crops.len();
            Ok(crops
                .iter()
                .map(|c| Prediction {
                    tile: if c.get_pixel(0, 0)[0] > 128 {
                        "3m"
                    } else {
                        "7p"
                    }
                    .parse()
                    .unwrap(),
                    confidence: self.confidence,
                })
                .collect())
        }
    }

    fn crop(width: u32, level: u8) -> RgbImage {
        RgbImage::from_fn(width, width * 4 / 3, |x, y| {
            if (y / 6) % 2 == 0 && x > 4 {
                Rgb([level / 4; 3])
            } else {
                Rgb([level; 3])
            }
        })
    }

    #[test]
    fn known_tiles_come_from_the_cache() {
        let mut recognizer = TileRecognizer::new(Fake {
            confidence: 0.9,
            asked: 0,
        });
        let hand = [crop(40, 240), crop(40, 60), crop(40, 240)];
        let first = recognizer.recognize(&hand).unwrap();
        assert_eq!(
            first.iter().map(|r| r.tile.to_string()).collect::<Vec<_>>(),
            ["3m", "7p", "3m"]
        );
        assert!(
            first
                .iter()
                .all(|r| r.source == Source::Classifier && r.doubt.is_none())
        );
        assert_eq!(recognizer.classifier.asked, 3);

        let second = recognizer.recognize(&hand).unwrap();
        assert!(second.iter().all(|r| r.source == Source::Cache));
        let cached: Vec<Recognition> = first
            .iter()
            .map(|r| Recognition {
                source: Source::Cache,
                ..*r
            })
            .collect();
        assert_eq!(second, cached);
        assert_eq!(recognizer.classifier.asked, 3, "no new classifier calls");

        recognizer.clear_cache();
        recognizer.recognize(&hand).unwrap();
        assert_eq!(recognizer.classifier.asked, 6);
    }

    #[test]
    fn the_same_doubtful_picture_is_not_asked_again() {
        let mut recognizer = TileRecognizer::new(Fake {
            confidence: 0.5,
            asked: 0,
        });
        let first = recognizer
            .recognize(&[crop(40, 240), crop(20, 60)])
            .unwrap();
        let again = recognizer
            .recognize(&[crop(40, 240), crop(20, 60)])
            .unwrap();
        assert_eq!(recognizer.classifier.asked, 2);
        assert_eq!(
            again.iter().map(|r| r.doubt).collect::<Vec<_>>(),
            [Some(Doubt::LowConfidence), Some(Doubt::TooSmall)]
        );
        assert_eq!(
            again.iter().map(|r| r.tile).collect::<Vec<_>>(),
            first.iter().map(|r| r.tile).collect::<Vec<_>>()
        );
    }

    #[test]
    fn small_or_unsure_tiles_are_in_doubt_and_not_cached() {
        let mut recognizer = TileRecognizer::new(Fake {
            confidence: 0.9,
            asked: 0,
        });
        let small = recognizer.recognize(&[crop(20, 240)]).unwrap();
        assert_eq!(small[0].doubt, Some(Doubt::TooSmall));
        assert!(
            recognizer
                .cache(Zone::Hand)
                .is_none_or(TemplateCache::is_empty)
        );

        let mut recognizer = TileRecognizer::new(Fake {
            confidence: 0.5,
            asked: 0,
        });
        let unsure = recognizer.recognize(&[crop(40, 240)]).unwrap();
        assert_eq!(unsure[0].doubt, Some(Doubt::LowConfidence));
        assert_eq!(unsure[0].tile.to_string(), "3m", "the best guess is kept");
        assert!(
            recognizer
                .cache(Zone::Hand)
                .is_none_or(TemplateCache::is_empty)
        );
    }
}
