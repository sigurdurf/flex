//! `ffprobe` invocation and mapping of its JSON output into protocol types.

use std::path::Path;

use flex_protocol::{AudioStream, SubtitleStream, VideoStream};
use serde::Deserialize;

use crate::model::StreamData;

/// Number that `ffprobe` may emit either as a JSON number or as a string.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum FlexNum {
    Num(i64),
    Str(String),
}

impl FlexNum {
    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Num(n) if *n >= 0 => Some(*n as u64),
            Self::Str(s) => s.parse::<u64>().ok(),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    #[serde(default)]
    format: Option<ProbeFormat>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeStream {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    codec_name: Option<String>,
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
    #[serde(default)]
    r_frame_rate: Option<String>,
    #[serde(default)]
    bit_rate: Option<FlexNum>,
    #[serde(default)]
    channels: Option<u32>,
    #[serde(default)]
    tags: Option<ProbeTags>,
    #[serde(default)]
    disposition: Option<ProbeDisposition>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeTags {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeDisposition {
    #[serde(default)]
    forced: Option<i64>,
    #[serde(default)]
    default: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct ProbeFormat {
    #[serde(default)]
    format_name: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    size: Option<FlexNum>,
}

/// The result of probing one file.
#[derive(Debug, Clone, Default)]
pub struct Probed {
    pub container: Option<String>,
    pub duration_ms: Option<u64>,
    pub file_size: Option<u64>,
    pub streams: StreamData,
}

fn parse_fraction(value: &str) -> Option<f64> {
    let (num, den) = value.split_once('/')?;
    let num: f64 = num.trim().parse().ok()?;
    let den: f64 = den.trim().parse().ok()?;
    if den == 0.0 || num == 0.0 {
        return None;
    }
    Some(num / den)
}

fn parse_duration_ms(value: &str) -> Option<u64> {
    let seconds: f64 = value.trim().parse().ok()?;
    if seconds <= 0.0 {
        return None;
    }
    Some((seconds * 1000.0).round() as u64)
}

fn clean_tag(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn map_probe(probe: Probe) -> Probed {
    let mut out = Probed::default();

    if let Some(format) = probe.format {
        out.container = clean_tag(format.format_name);
        out.duration_ms = format.duration.as_deref().and_then(parse_duration_ms);
        out.file_size = format.size.as_ref().and_then(FlexNum::as_u64);
    }

    for stream in probe.streams {
        let codec = stream.codec_name.unwrap_or_default();
        match stream.codec_type.as_deref() {
            Some("video") if out.streams.video.is_none() => {
                let fps = stream
                    .avg_frame_rate
                    .as_deref()
                    .and_then(parse_fraction)
                    .or_else(|| stream.r_frame_rate.as_deref().and_then(parse_fraction));
                out.streams.video = Some(VideoStream {
                    codec,
                    width: stream.width.unwrap_or(0),
                    height: stream.height.unwrap_or(0),
                    fps,
                    bitrate: stream.bit_rate.as_ref().and_then(FlexNum::as_u64),
                });
            }
            Some("audio") => {
                let tags = stream.tags.unwrap_or_default();
                out.streams.audio.push(AudioStream {
                    index: stream.index.unwrap_or(0),
                    codec,
                    channels: stream.channels,
                    language: clean_tag(tags.language),
                    title: clean_tag(tags.title),
                    bitrate: stream.bit_rate.as_ref().and_then(FlexNum::as_u64),
                });
            }
            Some("subtitle") => {
                let tags = stream.tags.unwrap_or_default();
                let disposition = stream.disposition.unwrap_or_default();
                out.streams.subtitles.push(SubtitleStream {
                    index: stream.index.unwrap_or(0),
                    codec,
                    language: clean_tag(tags.language),
                    title: clean_tag(tags.title),
                    forced: disposition.forced.unwrap_or(0) != 0,
                    default: disposition.default.unwrap_or(0) != 0,
                });
            }
            _ => {}
        }
    }

    out
}

/// Run `ffprobe` on `path` and map the result into protocol stream types.
pub async fn probe_file(path: &Path) -> anyhow::Result<Probed> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to run ffprobe: {e}"))?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe exited with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let probe: Probe = serde_json::from_slice(&output.stdout)
        .map_err(|e| anyhow::anyhow!("failed to parse ffprobe output: {e}"))?;

    let mut probed = map_probe(probe);
    if probed.file_size.is_none() {
        probed.file_size = tokio::fs::metadata(path).await.ok().map(|m| m.len());
    }
    Ok(probed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fractions() {
        assert_eq!(parse_fraction("25/1"), Some(25.0));
        assert_eq!(parse_fraction("24000/1001"), Some(24000.0 / 1001.0));
        assert_eq!(parse_fraction("0/0"), None);
        assert_eq!(parse_fraction("garbage"), None);
    }

    #[test]
    fn parses_duration_ms() {
        assert_eq!(parse_duration_ms("3.000000"), Some(3000));
        assert_eq!(parse_duration_ms("0"), None);
        assert_eq!(parse_duration_ms("nope"), None);
    }

    #[test]
    fn maps_streams() {
        let json = r#"{
            "streams": [
                {"index":0,"codec_name":"h264","codec_type":"video","width":320,"height":240,
                 "avg_frame_rate":"25/1","r_frame_rate":"25/1","bit_rate":"500000"},
                {"index":1,"codec_name":"aac","codec_type":"audio","channels":1,
                 "bit_rate":"128000","tags":{"language":"eng","title":"Stereo"}},
                {"index":2,"codec_name":"subrip","codec_type":"subtitle",
                 "tags":{"language":"eng"},"disposition":{"forced":0,"default":1}}
            ],
            "format": {"format_name":"mov,mp4,m4a,3gp,3g2,mj2","duration":"3.000000","size":"12345"}
        }"#;
        let probe: Probe = serde_json::from_str(json).unwrap();
        let probed = map_probe(probe);
        assert_eq!(probed.container.as_deref(), Some("mov,mp4,m4a,3gp,3g2,mj2"));
        assert_eq!(probed.duration_ms, Some(3000));
        assert_eq!(probed.file_size, Some(12345));
        let video = probed.streams.video.unwrap();
        assert_eq!(video.codec, "h264");
        assert_eq!(video.width, 320);
        assert_eq!(video.fps, Some(25.0));
        assert_eq!(probed.streams.audio.len(), 1);
        assert_eq!(probed.streams.audio[0].language.as_deref(), Some("eng"));
        assert_eq!(probed.streams.subtitles.len(), 1);
        assert!(probed.streams.subtitles[0].default);
        assert!(!probed.streams.subtitles[0].forced);
    }
}
