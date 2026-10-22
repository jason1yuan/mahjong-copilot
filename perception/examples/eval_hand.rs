//! Checks hand finding (#3) against annotated screenshots: whether each hand is found with
//! the right number of tiles and every annotated tile matched (overlap over 0.6), by
//! window width, and how long finding takes. Images stay local.
//!     cargo run -p mahjong-perception --release --example eval_hand -- recordings/source recordings/simulated_multi

use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mahjong_perception::capture::FrameView;
use mahjong_perception::hand::{HandFinder, TileBox};
use mahjong_perception::profile;
use serde::Deserialize;

#[derive(Deserialize)]
struct Annotation {
    image: Option<PathBuf>,
    #[serde(default)]
    ops: Vec<String>,
    hand: Vec<HandTile>,
}

#[derive(Deserialize)]
struct HandTile {
    r#box: [u32; 4],
}

fn overlap(a: [u32; 4], b: &TileBox) -> f32 {
    let w = a[2].min(b.right).saturating_sub(a[0].max(b.left));
    let h = a[3].min(b.bottom).saturating_sub(a[1].max(b.top));
    let inter = (w * h) as f32;
    let area = |l: u32, t: u32, r: u32, bo: u32| ((r - l) * (bo - t)) as f32;
    inter / (area(a[0], a[1], a[2], a[3]) + area(b.left, b.top, b.right, b.bottom) - inter)
}

fn main() -> Result<(), Box<dyn Error>> {
    let folders: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let profiles = profile::builtin()?;
    let face = profiles[0].tile_face;
    let mut finder = HandFinder::new();
    let mut by: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let mut seconds = Vec::new();
    for folder in &folders {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(folder)?
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        for json in paths {
            let Ok(annotation) =
                serde_json::from_str::<Annotation>(&std::fs::read_to_string(&json)?)
            else {
                continue;
            };
            let path = annotation
                .image
                .clone()
                .unwrap_or_else(|| json.with_extension("png"));
            let rgb = image::open(Path::new(&path))?.to_rgb8();
            let (w, h) = rgb.dimensions();
            let bgra: Vec<u8> = rgb.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
            let frame = FrameView::new(w, h, w as usize * 4, &bgra, 1.0).ok_or("bad frame")?;
            let start = Instant::now();
            let found = finder
                .find(&frame, face)
                .map(|l| l.tiles())
                .unwrap_or_default();
            seconds.push(start.elapsed().as_secs_f64());
            let want: Vec<[u32; 4]> = annotation.hand.iter().map(|t| t.r#box).collect();
            let matched = want
                .iter()
                .filter(|a| found.iter().any(|b| overlap(**a, b) > 0.6))
                .count();
            let ok = found.len() == want.len() && matched == want.len();
            let width = annotation
                .ops
                .iter()
                .find_map(|o| o.strip_prefix("width:"))
                .map_or("real".to_owned(), |w| format!("width {w:>4}"));
            let name = folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let big = annotation
                .ops
                .iter()
                .filter_map(|o| o.strip_prefix("width:"))
                .any(|w| w.parse::<u32>().is_ok_and(|w| w >= 960));
            let mut keys = vec![format!("{name} all"), format!("{name} {width}")];
            if big {
                keys.push(format!("{name} width >= 960"));
            }
            for key in keys {
                let entry = by.entry(key).or_default();
                entry.0 += u32::from(ok);
                entry.1 += 1;
            }
            if !ok {
                let stem = json
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                println!(
                    "  miss {stem}: want {} found {}, {matched} matched  {:?}",
                    want.len(),
                    found.len(),
                    annotation.ops
                );
            }
        }
    }
    for (key, (ok, n)) in &by {
        println!(
            "{key:32} {ok:3}/{n:<3} {:5.1}%",
            100.0 * *ok as f64 / *n as f64
        );
    }
    seconds.sort_by(f64::total_cmp);
    if let (Some(median), Some(max)) = (seconds.get(seconds.len() / 2), seconds.last()) {
        println!(
            "finding a hand: median {:.1} ms, max {:.1} ms",
            median * 1e3,
            max * 1e3
        );
    }
    Ok(())
}
