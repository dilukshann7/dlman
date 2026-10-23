//! Format normalization adapted from Rusty IDM; MP3 is an explicit conversion.
use super::{canonical_url, media_error};
use crate::DlmanError;
use dlman_types::{YoutubeOutput, YoutubePreview, YoutubeVariant};
use serde_json::Value;

fn clean(value: &str, limit: usize) -> String {
    let mut result = String::new();
    for character in value.chars().filter(|c| !c.is_control()) {
        if result.len() + character.len_utf8() > limit {
            break;
        }
        result.push(character);
    }
    result
}
fn id(value: &Value) -> Option<String> {
    let id = value.as_str()?;
    (id.len() <= 80
        && !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+')))
    .then(|| id.to_owned())
}
fn has_video(f: &Value) -> bool {
    f["vcodec"].as_str().is_some_and(|codec| codec != "none")
}
fn has_audio(f: &Value) -> bool {
    f["acodec"].as_str().is_some_and(|codec| codec != "none")
}
fn size(f: &Value) -> Option<u64> {
    f["filesize"]
        .as_u64()
        .or_else(|| f["filesize_approx"].as_u64())
}

const MAX_MEDIA_VARIANTS: usize = 128;

fn bitrate(f: &Value, field: &str) -> f64 {
    f[field]
        .as_f64()
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .or_else(|| {
            f["tbr"]
                .as_f64()
                .filter(|rate| rate.is_finite() && *rate > 0.0)
        })
        .unwrap_or(0.0)
}

fn compatible_audio(video_ext: &str, audio: &Value) -> bool {
    let extension = audio["ext"].as_str().unwrap_or("");
    let codec = audio["acodec"].as_str().unwrap_or("");
    match video_ext {
        "mp4" => {
            matches!(extension, "m4a" | "mp4") && (codec == "aac" || codec.starts_with("mp4a"))
        }
        "webm" => extension == "webm" && matches!(codec, "opus" | "vorbis"),
        _ => false,
    }
}

fn frame_rate(value: f64) -> Option<u32> {
    (value.is_finite() && value > 0.0 && value <= 1000.0).then(|| value.round().max(1.0) as u32)
}

fn quality_key(f: &Value) -> (u32, u32, String) {
    // YouTube's source label describes its quality tier even for cropped video.
    // Only accept a complete numeric p-label, not arbitrary format-note text.
    let note = f["format_note"].as_str().unwrap_or("");
    let source_label = note
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find_map(|token| {
            let (resolution, fps) = token.split_once('p')?;
            let resolution = resolution
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=16384).contains(n))?;
            let fps = if fps.is_empty() {
                None
            } else {
                Some(fps.parse::<u32>().ok().filter(|n| (1..=1000).contains(n))?)
            };
            Some((resolution, fps))
        });
    let height = f["height"].as_u64().filter(|n| (1..=16384).contains(n));
    let width = f["width"].as_u64().filter(|n| (1..=16384).contains(n));
    let resolution = source_label
        .map(|(resolution, _)| resolution)
        .unwrap_or_else(|| {
            height
                .map(|height| height.min(width.unwrap_or(height)) as u32)
                .unwrap_or(0)
        });
    let fps = f["fps"]
        .as_f64()
        .and_then(frame_rate)
        .or_else(|| source_label.and_then(|(_, fps)| fps))
        .unwrap_or(0);
    let quality = if resolution == 0 {
        let note = clean(note, 32);
        if note.is_empty() {
            "video".into()
        } else {
            note
        }
    } else if fps == 0 {
        format!("{resolution}p")
    } else {
        format!("{resolution}p{fps}")
    };
    (resolution, fps, quality)
}

fn container_rank(container: &str) -> u8 {
    match container {
        "mp4" => 0,
        "webm" => 1,
        _ => 2,
    }
}

struct PreviewCandidate {
    variant: YoutubeVariant,
    h264: bool,
    bitrate: f64,
}
impl PreviewCandidate {
    fn preference(&self, other: &Self) -> std::cmp::Ordering {
        container_rank(&self.variant.container)
            .cmp(&container_rank(&other.variant.container))
            .then_with(|| other.h264.cmp(&self.h264))
            .then_with(|| other.bitrate.total_cmp(&self.bitrate))
            .then_with(|| self.variant.id.cmp(&other.variant.id))
    }
}

pub fn parse_preview(url: &str, value: &Value) -> Result<YoutubePreview, DlmanError> {
    if !value.is_object()
        || value["entries"].is_array()
        || value["is_live"].as_bool() == Some(true)
        || matches!(
            value["live_status"].as_str(),
            Some("is_live" | "is_upcoming" | "post_live")
        )
        || matches!(
            value["availability"].as_str(),
            Some("private" | "premium_only" | "subscriber_only" | "needs_auth")
        )
    {
        return Err(media_error(
            "Only single public on-demand videos are supported.",
        ));
    }
    if value["has_drm"].as_bool() == Some(true) {
        return Err(media_error("DRM-protected videos are unsupported."));
    }
    let media_id =
        id(&value["id"]).ok_or_else(|| media_error("The media identity is unavailable."))?;
    if canonical_url(url)?.split("v=").nth(1) != Some(media_id.as_str()) {
        return Err(media_error("The inspected video identity changed."));
    }
    let identity = format!("Youtube:{media_id}");
    let title = clean(value["title"].as_str().unwrap_or("Video"), 160);
    let formats = value["formats"]
        .as_array()
        .ok_or_else(|| media_error("No video formats are available."))?;
    let audio_tracks: Vec<_> = formats
        .iter()
        .filter(|f| {
            !has_video(f)
                && has_audio(f)
                && f["has_drm"].as_bool() != Some(true)
                && id(&f["format_id"]).is_some()
        })
        .collect();
    let preferred_audio = |extension: &str| {
        audio_tracks.iter().copied().max_by(|a, b| {
            compatible_audio(extension, a)
                .cmp(&compatible_audio(extension, b))
                .then_with(|| bitrate(a, "abr").total_cmp(&bitrate(b, "abr")))
                .then_with(|| b["format_id"].as_str().cmp(&a["format_id"].as_str()))
        })
    };
    let mp4_audio = preferred_audio("mp4");
    let webm_audio = preferred_audio("webm");
    let fallback_audio = preferred_audio("");
    let mp3_source = fallback_audio.or_else(|| {
        formats
            .iter()
            .filter(|f| {
                has_audio(f)
                    && f["has_drm"].as_bool() != Some(true)
                    && id(&f["format_id"]).is_some()
            })
            .max_by(|a, b| bitrate(a, "abr").total_cmp(&bitrate(b, "abr")))
    });
    let mut candidates = std::collections::BTreeMap::new();
    for f in formats
        .iter()
        .filter(|f| has_video(f) && f["has_drm"].as_bool() != Some(true))
    {
        let Some(video_id) = id(&f["format_id"]) else {
            continue;
        };
        let extension = f["ext"].as_str().unwrap_or("mkv");
        let best_audio = match extension {
            "mp4" => mp4_audio,
            "webm" => webm_audio,
            _ => fallback_audio,
        };
        let (selector, container, audio, bytes) = if has_audio(f) {
            (
                video_id,
                extension.to_owned(),
                clean(f["acodec"].as_str().unwrap_or("unknown"), 32),
                size(f),
            )
        } else if let Some(track) = best_audio {
            let audio_id = id(&track["format_id"]).expect("filtered");
            let container = if compatible_audio(extension, track) {
                extension
            } else {
                "mkv"
            };
            (
                format!("{video_id}+{audio_id}"),
                container.into(),
                clean(track["acodec"].as_str().unwrap_or("audio"), 32),
                size(f).zip(size(track)).map(|(a, b)| a.saturating_add(b)),
            )
        } else {
            continue;
        };
        if selector.len() > 96 || !["mp4", "webm", "mkv"].contains(&container.as_str()) {
            continue;
        }
        let (resolution, fps, quality) = quality_key(f);
        let codec = f["vcodec"].as_str().unwrap_or("");
        let candidate = PreviewCandidate {
            variant: YoutubeVariant {
                id: selector,
                quality: quality.clone(),
                container,
                audio,
                size: bytes,
                fps: f["fps"].as_f64().and_then(frame_rate),
                output_mode: YoutubeOutput::Video,
            },
            h264: codec == "h264" || codec.starts_with("avc1") || codec.starts_with("avc3"),
            bitrate: bitrate(f, "vbr"),
        };
        let key = (resolution, fps, quality);
        match candidates.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if candidate.preference(entry.get()).is_lt() {
                    entry.insert(candidate);
                }
            }
        }
    }
    if candidates.len() > MAX_MEDIA_VARIANTS {
        return Err(media_error("Too many video quality choices are available."));
    }
    let mut candidates: Vec<_> = candidates.into_iter().collect();
    candidates.sort_by_key(|((resolution, fps, quality), candidate)| {
        (
            candidate.variant.container != "mp4",
            std::cmp::Reverse(*resolution),
            std::cmp::Reverse(*fps),
            quality.clone(),
        )
    });
    let mut variants: Vec<_> = candidates
        .into_iter()
        .map(|(_, candidate)| candidate.variant)
        .collect();
    if let Some(audio) = mp3_source {
        variants.push(YoutubeVariant {
            id: id(&audio["format_id"]).expect("filtered"),
            quality: "Audio only · MP3".into(),
            container: "mp3".into(),
            audio: "mp3".into(),
            size: None,
            fps: None,
            output_mode: YoutubeOutput::Mp3,
        });
    }
    if variants.len() > MAX_MEDIA_VARIANTS {
        return Err(media_error("Too many media choices are available."));
    }
    if variants.is_empty() {
        return Err(media_error(
            "No supported video and audio format pair is available.",
        ));
    }
    Ok(YoutubePreview {
        url: url.into(),
        identity,
        title,
        variants,
    })
}
