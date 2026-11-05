//! The hub, as the phone's store sees it: the public catalogue, each app's
//! screenshot made ready to draw, and the app's bytes when someone taps Get.
//!
//! Everything is cached in the app's own folder, so Discover still shows
//! the last catalogue when the phone is offline, and a screenshot is
//! fetched and decoded once.
//!
//! The Android player includes this same file (crates/player-android), so
//! both phones show one Discover.

#![cfg(any(target_os = "ios", target_os = "android"))]

use std::io::Read;
use std::path::Path;
use std::time::Duration;

const HUB: &str = "https://hub.krate.tech";
/// Wider than any card on a phone at 2x; anything bigger is decoded down.
const ART_WIDTH: u32 = 640;

/// One app on the hub.
pub struct HubApp {
    pub id: String,
    pub name: String,
    pub author: String,
    pub category: String,
    pub description: String,
    pub shot: Option<String>,
    pub size: u64,
}

#[cfg(target_os = "ios")]
const USER_AGENT: &str = concat!("Krate/", env!("CARGO_PKG_VERSION"), " (iPhone)");
#[cfg(target_os = "android")]
const USER_AGENT: &str = concat!("Krate/", env!("CARGO_PKG_VERSION"), " (Android)");

fn agent() -> ureq::Agent {
    // The hub's edge refuses requests without a real user agent.
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .user_agent(USER_AGENT)
        .build()
}

fn get(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let response = agent().get(url).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("larger than expected".to_string());
    }
    Ok(bytes)
}

/// Fetch the catalogue and any screenshot not yet decoded, into the cache,
/// for the next time the home screen is drawn. Slow (the network), so the
/// home screen never waits on it except the very first time.
pub fn refresh(cache: &Path, art_dir: &Path) {
    match get(&format!("{HUB}/apps"), 8 * 1024 * 1024) {
        Ok(bytes) => {
            // Written whole, then moved into place: a reader never sees half.
            let part = cache.join("catalogue.json.part");
            if std::fs::write(&part, &bytes).is_ok() {
                let _ = std::fs::rename(&part, cache.join("catalogue.json"));
            }
        }
        Err(why) => eprintln!("hub catalogue: {why}; keeping the last one"),
    }
    // Every screenshot at once: one after another, a first visit waited
    // on a dozen round trips in a row.
    let apps = catalogue(cache);
    std::thread::scope(|scope| {
        for app in &apps {
            scope.spawn(move || art(app, art_dir));
        }
    });
}

/// The listed apps, as last fetched; empty before the first fetch.
pub fn catalogue(cache: &Path) -> Vec<HubApp> {
    let text = std::fs::read_to_string(cache.join("catalogue.json")).unwrap_or_default();
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let text_of = |v: &serde_json::Value, key: &str| {
        v.get(key)
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .replace(['\t', '\n', '\r'], " ")
    };
    json.get("apps")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|app| {
            let meta = app.get("meta")?;
            Some(HubApp {
                id: app.get("id")?.as_str()?.to_string(),
                name: text_of(meta, "name"),
                author: text_of(meta, "author"),
                category: text_of(meta, "category"),
                description: text_of(meta, "description"),
                shot: app.get("shot").and_then(|s| s.as_str()).map(str::to_string),
                size: meta.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
            })
        })
        .filter(|app| !app.name.is_empty())
        .collect()
}

fn art_name(app: &HubApp) -> String {
    format!("{}.rgba", &app.id[..app.id.len().min(16)])
}

/// The app's screenshot file, if it has been fetched and decoded already.
pub fn cached_art(app: &HubApp, art_dir: &Path) -> Option<String> {
    let name = art_name(app);
    art_dir.join(&name).exists().then_some(name)
}

/// The app's screenshot as a ready-to-draw file in `art_dir`, fetched and
/// decoded the first time. Returns its file name.
pub fn art(app: &HubApp, art_dir: &Path) -> Option<String> {
    let name = art_name(app);
    if art_dir.join(&name).exists() {
        return Some(name);
    }
    let png = get(app.shot.as_deref()?, 8 * 1024 * 1024).ok()?;
    let (width, height, rgba) = decode_png(&png)?;
    let (width, height, rgba) = shrink(width, height, &rgba, ART_WIDTH);
    let mut file = Vec::with_capacity(8 + rgba.len());
    file.extend_from_slice(&width.to_le_bytes());
    file.extend_from_slice(&height.to_le_bytes());
    file.extend_from_slice(&rgba);
    std::fs::write(art_dir.join(&name), file).ok()?;
    Some(name)
}

/// The app's `.krate`, from its hub link.
pub fn download(id: &str, limit: u64) -> Result<Vec<u8>, String> {
    get(&format!("{HUB}/a/{id}"), limit)
}

fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let pixels = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

/// Average down by a whole factor until the width fits.
fn shrink(width: u32, height: u32, rgba: &[u8], max: u32) -> (u32, u32, Vec<u8>) {
    let factor = width.div_ceil(max).max(1);
    if factor == 1 {
        return (width, height, rgba.to_vec());
    }
    let (w, h) = (width / factor, height / factor);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 4];
            for dy in 0..factor {
                for dx in 0..factor {
                    let i = (((y * factor + dy) * width + x * factor + dx) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += rgba[i + c] as u32;
                    }
                }
            }
            let n = factor * factor;
            out.extend(sum.iter().map(|s| (s / n) as u8));
        }
    }
    (w, h, out)
}
