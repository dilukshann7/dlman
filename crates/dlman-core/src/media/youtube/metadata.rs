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
fn positive_number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite() && *n > 0.0)
}

pub(super) fn positive_bytes(value: &Value) -> Option<u64> {
    if let Some(bytes) = value.as_u64() {
        return (bytes > 0 && bytes <= i64::MAX as u64).then_some(bytes);
    }
    positive_number(value)
        .filter(|bytes| bytes.ceil() < i64::MAX as f64)
        .map(|bytes| bytes.ceil() as u64)
}

fn size(f: &Value, duration: Option<f64>) -> Option<u64> {
    positive_bytes(&f["filesize"])
        .or_else(|| positive_bytes(&f["filesize_approx"]))
        .or_else(|| {
            let duration = positive_number(&f["duration"]).or(duration)?;
            let rate =
                positive_number(&f["tbr"]).or_else(|| match (has_video(f), has_audio(f)) {
                    (true, true) => Some(positive_number(&f["vbr"])? + positive_number(&f["abr"])?),
                    (true, false) => positive_number(&f["vbr"]),
                    (false, true) => positive_number(&f["abr"]),
                    _ => None,
                })?;
            // yt-dlp bitrates are decimal kilobits per second; this is an estimate.
            let bytes = duration * rate * 1000.0 / 8.0;
            (bytes.is_finite() && bytes > 0.0 && bytes.ceil() < i64::MAX as f64)
                .then(|| bytes.ceil() as u64)
        })
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
    let media_id =
        id(&value["id"]).ok_or_else(|| media_error("The media identity is unavailable."))?;
    if canonical_url(url)?.split("v=").nth(1) != Some(media_id.as_str()) {
        return Err(media_error("The inspected video identity changed."));
    }
    parse_formats(url, value, format!("Youtube:{media_id}"), true)
}

/// Generic site identities must not be validated as YouTube video IDs.
pub fn parse_web_preview(url: &str, value: &Value) -> Result<YoutubePreview, DlmanError> {
    let parsed = url::Url::parse(url).map_err(|_| media_error("Invalid video webpage URL."))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || super::is_youtube_host(url)
    {
        return Err(media_error("Use a non-YouTube HTTP or HTTPS video URL."));
    }
    let media_id = value["id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .ok_or_else(|| media_error("The media identity is unavailable."))?;
    let extractor = value["extractor_key"]
        .as_str()
        .or_else(|| value["extractor"].as_str())
        .unwrap_or("Generic");
    if extractor.to_ascii_lowercase().starts_with("youtube") {
        return Err(media_error(
            "Open this embedded video on YouTube and paste its video link.",
        ));
    }
    if extractor.len() > 128
        || media_id
            .chars()
            .chain(extractor.chars())
            .any(char::is_control)
    {
        return Err(media_error("Invalid media identity."));
    }
    // Progressive HTML5/KVS formats often contain no codec information at all.
    // Preserve unknown codecs as unknown; requiring an explicit vcodec discarded
    // every quality from these otherwise valid video player configurations.
    let mut normalized = value.clone();
    if let Some(formats) = normalized["formats"].as_array_mut() {
        for format in formats {
            normalize_web_progressive(format);
        }
    } else {
        normalize_web_progressive(&mut normalized);
    }
    parse_formats(url, &normalized, format!("{extractor}:{media_id}"), false)
}

fn normalize_web_progressive(format: &mut Value) {
    let direct = format["protocol"]
        .as_str()
        .is_none_or(|protocol| matches!(protocol, "http" | "https"))
        && format["url"]
            .as_str()
            .and_then(|value| url::Url::parse(value).ok())
            .is_some_and(|url| matches!(url.scheme(), "http" | "https"));
    let video_container = matches!(
        format["ext"].as_str(),
        Some("mp4" | "m4v" | "mov" | "webm" | "mkv" | "flv" | "ogv")
    );
    if !direct || !video_container || format["vcodec"].as_str() == Some("none") {
        return;
    }
    if format["vcodec"].is_null() {
        format["vcodec"] = Value::String("unknown".into());
    }
    if format["acodec"].is_null() {
        // Treat progressive files as muxed while their audio codec is unknown.
        // Do not append another audio track or classify them as silent.
        format["acodec"] = Value::String("unknown".into());
    }
    if format["format_note"].as_str().is_none_or(str::is_empty) {
        if let Some(label) = format["format_id"].as_str().map(str::to_owned) {
            format["format_note"] = Value::String(label);
        }
    }
}

fn parse_formats(
    url: &str,
    value: &Value,
    identity: String,
    youtube: bool,
) -> Result<YoutubePreview, DlmanError> {
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
    let title = clean(value["title"].as_str().unwrap_or("Video"), 160);
    let duration = positive_number(&value["duration"]);
    let formats = value["formats"]
        .as_array()
        .map(Vec::as_slice)
        .or_else(|| value["url"].as_str().map(|_| std::slice::from_ref(value)))
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
        let source_extension = f["ext"].as_str().unwrap_or("mkv");
        let extension = if youtube {
            source_extension
        } else {
            match source_extension {
                "mp4" | "m4v" | "ts" | "m3u8" => "mp4",
                "webm" => "webm",
                _ => "mkv",
            }
        };
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
                size(f, duration),
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
                size(f, duration)
                    .zip(size(track, duration))
                    .and_then(|(a, b)| a.checked_add(b))
                    .filter(|bytes| *bytes <= i64::MAX as u64),
            )
        } else if !youtube && !formats.iter().any(|format| has_audio(format)) {
            // A genuinely silent clip is still a valid video; do not manufacture audio.
            (
                video_id,
                extension.to_owned(),
                "none".into(),
                size(f, duration),
            )
        } else {
            continue;
        };
        if selector.len() > 96 || !["mp4", "webm", "mkv"].contains(&container.as_str()) {
            continue;
        }
        let (resolution, fps, mut quality) = quality_key(f);
        if !youtube && resolution == 0 && quality == "video" {
            let rate = bitrate(f, "vbr");
            quality = if rate > 0.0 {
                format!("{rate:.0} kbps")
            } else {
                format!("Video · {selector}")
            };
        }
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
            youtube && candidate.variant.container != "mp4",
            std::cmp::Reverse(*resolution),
            std::cmp::Reverse(*fps),
            candidate.variant.container != "mp4",
            quality.clone(),
        )
    });
    let mut variants: Vec<_> = candidates
        .into_iter()
        .map(|(_, candidate)| candidate.variant)
        .collect();
    if let Some(audio) = mp3_source.filter(|_| youtube) {
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

#[cfg(test)]
mod web_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kvs_progressive_formats_without_codec_metadata_keep_all_labeled_qualities() {
        let value = json!({"id":"42", "extractor_key":"Generic", "title":"Example clip",
        "formats":[
            {"format_id":"1080p", "ext":"mp4", "height":1080,
                "url":"https://cdn.example/get_file/42.mp4/?v-acctoken=old"},
            {"format_id":"720p", "ext":"mp4", "height":720,
                "url":"https://cdn.example/get_file/42_720p.mp4/?v-acctoken=old"},
            {"format_id":"360p", "ext":"mp4", "height":360,
                "url":"https://cdn.example/get_file/42_360p.mp4/?v-acctoken=old"}
        ]});
        let preview = parse_web_preview("https://video.example/watch/42", &value).unwrap();
        assert_eq!(preview.identity, "Generic:42");
        assert_eq!(
            preview
                .variants
                .iter()
                .map(|v| v.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1080p", "720p", "360p"]
        );
        assert!(preview
            .variants
            .iter()
            .all(|v| v.container == "mp4" && v.audio == "unknown"));
        let mut refreshed = value.clone();
        refreshed["formats"][1]["url"] =
            json!("https://cdn.example/get_file/42_720p.mp4/?v-acctoken=new");
        assert_eq!(
            serde_json::to_value(
                parse_web_preview("https://video.example/watch/42", &refreshed).unwrap()
            )
            .unwrap(),
            serde_json::to_value(preview).unwrap()
        );
    }

    #[test]
    fn html5_formats_use_labels_when_dimensions_and_codecs_are_missing() {
        let value = json!({"id":"html5", "extractor_key":"Generic", "formats":[
            {"format_id":"720p60", "ext":"mp4", "protocol":"https", "vcodec":null, "acodec":null,
                "url":"https://video.example/get_file/42_720m.mp4/"},
            {"format_id":"480p", "ext":"mp4", "vcodec":null,
                "url":"https://video.example/get_file/42_480m.mp4/"},
            {"format_id":"audio", "ext":"mp4", "vcodec":"none", "acodec":"aac",
                "url":"https://video.example/audio.mp4"}
        ]});
        let preview = parse_web_preview("https://video.example/watch/42", &value).unwrap();
        assert_eq!(
            preview
                .variants
                .iter()
                .map(|v| v.quality.as_str())
                .collect::<Vec<_>>(),
            vec!["720p60", "480p"]
        );
        assert_eq!(preview.variants[0].id, "720p60");
        assert!(preview.variants.iter().all(|v| v.audio == "unknown"));
    }

    #[test]
    fn inferred_progressive_codecs_do_not_change_explicit_silent_or_adaptive_tracks() {
        let value = json!({"id":"silent", "formats":[
            {"format_id":"only", "ext":"mp4", "height":720, "acodec":"none",
                "url":"https://cdn.example/silent.mp4"}
        ]});
        let preview = parse_web_preview("https://video.example/silent", &value).unwrap();
        assert_eq!(preview.variants[0].audio, "none");
        let unsupported = json!({"id":"segments", "formats":[
            {"format_id":"unknown", "ext":"mp4", "protocol":"http_dash_segments",
                "url":"https://cdn.example/track.mp4"}
        ]});
        assert!(parse_web_preview("https://video.example/watch", &unsupported).is_err());
    }

    fn fixture() -> Value {
        json!({"id":"site/video:123", "extractor_key":"Generic", "title":"Embedded clip", "duration":10,
        "formats":[
            {"format_id":"hls-720", "ext":"mp4", "height":720, "width":1280, "fps":60,
                "vcodec":"avc1", "acodec":"none", "tbr":1000},
            {"format_id":"dash-1080", "ext":"webm", "height":1080, "width":1920, "fps":30,
                "vcodec":"vp9", "acodec":"none", "tbr":2000},
            {"format_id":"aac", "ext":"m4a", "vcodec":"none", "acodec":"mp4a.40.2", "abr":128},
            {"format_id":"opus", "ext":"webm", "vcodec":"none", "acodec":"opus", "abr":160}
        ]})
    }

    #[test]
    fn generic_identity_and_adaptive_audio_preserve_highest_non_mp4_quality() {
        let preview = parse_web_preview("https://example.com/watch/123", &fixture()).unwrap();
        assert_eq!(preview.identity, "Generic:site/video:123");
        assert_eq!(preview.variants.len(), 2);
        assert_eq!(preview.variants[0].quality, "1080p30");
        assert_eq!(preview.variants[0].id, "dash-1080+opus");
        assert_eq!(preview.variants[1].id, "hls-720+aac");
        assert!(preview
            .variants
            .iter()
            .all(|v| v.output_mode == YoutubeOutput::Video));
    }

    #[test]
    fn silent_clip_is_available_without_creating_an_audio_track() {
        let value = json!({"id":"silent", "extractor":"generic", "formats":[
            {"format_id":"only", "ext":"mp4", "height":720, "vcodec":"h264", "acodec":"none"}
        ]});
        let preview = parse_web_preview("https://example.com/silent", &value).unwrap();
        assert_eq!(preview.variants.len(), 1);
        assert_eq!(preview.variants[0].audio, "none");
    }

    #[test]
    fn preserves_frame_rates_and_prefers_mp4_for_equivalent_quality() {
        let value = json!({"id":"clip", "formats":[
            {"format_id":"w30", "ext":"webm", "height":1080, "fps":30, "vcodec":"vp9", "acodec":"opus"},
            {"format_id":"m30", "ext":"mp4", "height":1080, "fps":30, "vcodec":"h264", "acodec":"aac"},
            {"format_id":"m60", "ext":"mp4", "height":1080, "fps":60, "vcodec":"h264", "acodec":"aac"}
        ]});
        let preview = parse_web_preview("https://example.com/clip", &value).unwrap();
        assert_eq!(
            preview
                .variants
                .iter()
                .map(|v| v.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m60", "m30"]
        );
    }

    #[test]
    fn rejects_live_playlists_drm_accounts_and_youtube_without_silent_fallback() {
        for (field, restriction) in [
            ("is_live", json!(true)),
            ("entries", json!([])),
            ("has_drm", json!(true)),
            ("availability", json!("needs_auth")),
            ("extractor_key", json!("Youtube")),
        ] {
            let mut value = fixture();
            value[field] = restriction;
            assert!(
                parse_web_preview("https://example.com/video", &value).is_err(),
                "{field}"
            );
        }
        for url in [
            "file:///private/video",
            "https://user:secret@example.com/video",
            "https://www.youtube.com/watch?v=abcdefghijk",
        ] {
            assert!(parse_web_preview(url, &fixture()).is_err());
        }
    }

    #[test]
    fn bandwidth_only_manifests_keep_distinct_quality_choices() {
        let value = json!({"id":"master", "formats":[
            {"format_id":"lo", "ext":"ts", "tbr":500, "vcodec":"h264", "acodec":"aac"},
            {"format_id":"hi", "ext":"ts", "tbr":1500, "vcodec":"h264", "acodec":"aac"}
        ]});
        let preview = parse_web_preview("https://example.com/manifest?token=old", &value).unwrap();
        assert_eq!(preview.variants.len(), 2);
        assert!(preview.variants.iter().all(|v| v.container == "mp4"));
    }
}
