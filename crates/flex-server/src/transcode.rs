//! `ffmpeg` capability detection and HLS argument construction.
//!
//! The argument builders are pure functions so the exact command line Flex
//! issues can be unit-tested without spawning a process. [`spawn`] is the only
//! part that touches the outside world.

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// Encoders considered hardware accelerators, in rough preference order.
const HW_ENCODER_SUFFIXES: &[&str] = &[
    "_nvenc",
    "_vaapi",
    "_qsv",
    "_amf",
    "_v4l2m2m",
    "_videotoolbox",
    "_mediacodec",
    "_mf",
];

/// Hardware accel methods that can decode with a simple `-hwaccel <name>` flag.
const PREFERRED_HWACCELS: &[&str] = &["cuda", "vaapi", "qsv", "d3d11va", "videotoolbox"];

/// The `ffmpeg` build's capabilities, probed once at startup.
#[derive(Debug, Clone)]
pub struct TranscodeCapabilities {
    /// Methods reported by `ffmpeg -hwaccels`.
    pub hwaccels: Vec<String>,
    /// Hardware encoder names reported by `ffmpeg -encoders`.
    pub hw_encoders: Vec<String>,
    /// Chosen H.264 encoder (hardware when available, else `libx264`).
    pub h264_encoder: String,
    /// Chosen HEVC encoder (hardware when available, else `libx265`).
    pub hevc_encoder: String,
    /// Hardware accel method to pass as `-hwaccel`, if any.
    pub hwaccel: Option<String>,
    /// Whether `ffmpeg` was found and understood.
    pub available: bool,
}

impl Default for TranscodeCapabilities {
    fn default() -> Self {
        Self {
            hwaccels: Vec::new(),
            hw_encoders: Vec::new(),
            h264_encoder: "libx264".to_string(),
            hevc_encoder: "libx265".to_string(),
            hwaccel: None,
            available: false,
        }
    }
}

impl TranscodeCapabilities {
    /// Build capabilities from already-parsed encoder/hwaccel lists.
    ///
    /// Exposed so tests can construct a deterministic plan without probing.
    pub fn from_lists(hwaccels: Vec<String>, encoders: Vec<String>) -> Self {
        let hw_encoders: Vec<String> = encoders
            .iter()
            .filter(|name| {
                HW_ENCODER_SUFFIXES
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
            })
            .cloned()
            .collect();

        let h264_encoder = first_present(
            &encoders,
            &["h264_nvenc", "h264_vaapi", "h264_qsv", "h264_amf"],
        )
        .unwrap_or("libx264")
        .to_string();
        let hevc_encoder = first_present(
            &encoders,
            &["hevc_nvenc", "hevc_vaapi", "hevc_qsv", "hevc_amf"],
        )
        .unwrap_or("libx265")
        .to_string();
        let hwaccel = PREFERRED_HWACCELS
            .iter()
            .find(|hw| hwaccels.iter().any(|h| h == *hw))
            .map(|s| s.to_string());

        Self {
            hwaccels,
            hw_encoders,
            h264_encoder,
            hevc_encoder,
            hwaccel,
            available: true,
        }
    }

    /// Whether `encoder` is a hardware encoder.
    pub fn is_hardware(encoder: &str) -> bool {
        HW_ENCODER_SUFFIXES
            .iter()
            .any(|suffix| encoder.ends_with(suffix))
    }
}

fn first_present<'a>(list: &[String], candidates: &[&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .copied()
        .find(|candidate| list.iter().any(|item| item == candidate))
}

/// Parse the output of `ffmpeg -hwaccels`.
fn parse_hwaccels(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("Hardware acceleration"))
        .map(|line| line.to_string())
        .collect()
}

/// Parse the output of `ffmpeg -hide_banner -encoders`.
fn parse_encoders(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let flags = parts.next()?;
            // Encoder rows start with a 6-character flag field (e.g. `V....D`),
            // the first character being the codec type (`V`/`A`/`S`).
            if flags.len() != 6 || !flags.starts_with(['V', 'A', 'S']) {
                return None;
            }
            let name = parts.next()?;
            // Skip the legend rows (`V..... = Video`).
            if name == "=" || !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
                return None;
            }
            Some(name.to_string())
        })
        .collect()
}

/// Probe `ffmpeg` for hardware acceleration and encoder support.
///
/// Never fails: a missing/broken `ffmpeg` yields an unavailable capability set.
pub async fn detect() -> TranscodeCapabilities {
    let hwaccels = run_ffmpeg(&["-hide_banner", "-hwaccels"]).await;
    let encoders = run_ffmpeg(&["-hide_banner", "-encoders"]).await;

    match (hwaccels, encoders) {
        (Some(hw), Some(enc)) => {
            let mut caps =
                TranscodeCapabilities::from_lists(parse_hwaccels(&hw), parse_encoders(&enc));
            caps.available = true;
            caps
        }
        _ => TranscodeCapabilities::default(),
    }
}

async fn run_ffmpeg(args: &[&str]) -> Option<String> {
    let output = Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// HLS segment container and filename for a given output video codec.
///
/// fMP4 (init + `.m4s`) is used for HEVC/AV1; MPEG-TS (`.ts`) for H.264 and
/// anything else.
pub fn hls_segment_type_for(video_codec: &str) -> (&'static str, &'static str) {
    match video_codec.to_ascii_lowercase().as_str() {
        "hevc" | "h265" | "av1" => ("fmp4", "seg_%05d.m4s"),
        _ => ("mpegts", "seg_%05d.ts"),
    }
}

/// Format a millisecond offset for `-ss` (fractional seconds).
fn seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// Push the shared HLS output options onto `args`.
fn push_hls(args: &mut Vec<String>, segment_type: &str, segment_name: &str) {
    args.extend(
        ["-f", "hls", "-hls_segment_type", segment_type]
            .iter()
            .map(|s| s.to_string()),
    );
    if segment_type == "fmp4" {
        args.extend(
            ["-hls_fmp4_init_filename", "init.mp4"]
                .iter()
                .map(|s| s.to_string()),
        );
    }
    args.extend(
        [
            "-hls_time",
            "6",
            "-hls_playlist_type",
            "event",
            "-hls_flags",
            "independent_segments+append_list",
            "-hls_segment_filename",
            segment_name,
            "master.m3u8",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
}

/// Build the `ffmpeg` argument vector for a **remux** session.
///
/// Video is copied verbatim; audio is copied when `copy_audio`, otherwise it is
/// re-encoded to AAC 192k stereo (for clients that cannot decode the source).
pub fn remux_args(
    input: &str,
    video_codec: &str,
    copy_audio: bool,
    start_ms: Option<u64>,
) -> Vec<String> {
    let (segment_type, segment_name) = hls_segment_type_for(video_codec);
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into()];
    if let Some(ms) = start_ms {
        args.push("-ss".into());
        args.push(seconds(ms));
    }
    args.extend(["-i".into(), input.to_string()]);
    args.extend(
        ["-map", "0:v:0", "-c:v", "copy", "-map", "0:a:0"]
            .iter()
            .map(|s| s.to_string()),
    );
    if copy_audio {
        args.extend(["-c:a".into(), "copy".into()]);
    } else {
        args.extend(
            ["-c:a", "aac", "-b:a", "192k", "-ac", "2"]
                .iter()
                .map(|s| s.to_string()),
        );
    }
    push_hls(&mut args, segment_type, segment_name);
    args
}

/// Build the `,`-free scale filter for the client's maximum dimensions.
pub fn scale_filter(max_width: Option<u32>, max_height: Option<u32>) -> Option<String> {
    match (max_width, max_height) {
        (Some(w), Some(h)) => Some(format!(
            "scale=w='min({w},iw)':h='min({h},ih)':force_original_aspect_ratio=decrease:force_divisible_by=2"
        )),
        (Some(w), None) => Some(format!("scale={w}:-2")),
        (None, Some(h)) => Some(format!("scale=-2:{h}")),
        (None, None) => None,
    }
}

/// Video bitrate triplet `(bitrate, maxrate, bufsize)`.
pub fn bitrate_args(max_bitrate: Option<u64>) -> (String, String, String) {
    match max_bitrate {
        Some(b) => {
            let maxrate = b + b / 2;
            let bufsize = b * 2;
            (b.to_string(), maxrate.to_string(), bufsize.to_string())
        }
        None => ("8M".into(), "12M".into(), "16M".into()),
    }
}

/// Build the `ffmpeg` argument vector for a **transcode** session.
///
/// Uses a hardware encoder (NVENC first) when available, otherwise `libx264`.
pub fn transcode_args(
    caps: &TranscodeCapabilities,
    input: &str,
    want_hevc: bool,
    scale: Option<&str>,
    max_bitrate: Option<u64>,
    start_ms: Option<u64>,
) -> Vec<String> {
    let encoder = if want_hevc {
        caps.hevc_encoder.as_str()
    } else {
        caps.h264_encoder.as_str()
    };
    let segment_type = if want_hevc { "fmp4" } else { "mpegts" };
    let segment_name = if want_hevc {
        "seg_%05d.m4s"
    } else {
        "seg_%05d.ts"
    };

    let hardware = TranscodeCapabilities::is_hardware(encoder);
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into()];
    if let Some(ms) = start_ms {
        args.push("-ss".into());
        args.push(seconds(ms));
    }
    if hardware {
        if let Some(hwaccel) = &caps.hwaccel {
            args.push("-hwaccel".into());
            args.push(hwaccel.clone());
        }
    }
    args.extend(["-i".into(), input.to_string()]);
    args.extend(["-map".into(), "0:v:0".into(), "-map".into(), "0:a:0".into()]);
    args.extend(["-c:v".into(), encoder.to_string()]);

    if hardware {
        args.extend(
            ["-preset", "p4", "-tune", "hq", "-rc", "vbr", "-cq", "23"]
                .iter()
                .map(|s| s.to_string()),
        );
    } else {
        args.extend(
            ["-preset", "veryfast", "-crf", "23"]
                .iter()
                .map(|s| s.to_string()),
        );
    }

    let (bitrate, maxrate, bufsize) = bitrate_args(max_bitrate);
    args.extend(
        ["-b:v", &bitrate, "-maxrate", &maxrate, "-bufsize", &bufsize]
            .iter()
            .map(|s| s.to_string()),
    );

    if let Some(scale) = scale {
        args.push("-vf".into());
        args.push(scale.to_string());
    }

    args.extend(
        ["-c:a", "aac", "-b:a", "192k", "-ac", "2"]
            .iter()
            .map(|s| s.to_string()),
    );
    push_hls(&mut args, segment_type, segment_name);
    args
}

/// Spawn `ffmpeg` in `session_dir`, returning the child handle.
///
/// `stderr` is piped so the caller can forward diagnostics; the caller is
/// expected to drain it (see [`log_stderr`]).
pub fn spawn(session_dir: &Path, args: &[String]) -> std::io::Result<Child> {
    Command::new("ffmpeg")
        .args(args)
        .current_dir(session_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

/// Forward a child's `stderr` to the tracing log, tagged with `session_id`.
pub fn log_stderr(child: &mut Child, session_id: String) {
    let Some(stderr) = child.stderr.take() else {
        return;
    };
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(session = %session_id, "ffmpeg: {line}");
        }
    });
}

/// Terminate a child cleanly, waiting for it to exit.
pub async fn kill(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvenc_caps() -> TranscodeCapabilities {
        TranscodeCapabilities::from_lists(
            vec!["vdpau".into(), "cuda".into(), "vaapi".into()],
            vec![
                "libx264".into(),
                "h264_nvenc".into(),
                "hevc_nvenc".into(),
                "aac".into(),
            ],
        )
    }

    fn software_caps() -> TranscodeCapabilities {
        TranscodeCapabilities::from_lists(vec![], vec!["libx264".into()])
    }

    #[test]
    fn parses_hwaccels_and_encoders() {
        assert_eq!(
            parse_hwaccels("Hardware acceleration methods:\nvdpau\ncuda\n"),
            vec!["vdpau", "cuda"]
        );
        let encoders = parse_encoders(
            "Encoders:\n V..... = Video\n ------\n V....D h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)\n A..... aac                  AAC (Advanced Audio Coding)\n",
        );
        assert_eq!(encoders, vec!["h264_nvenc", "aac"]);
    }

    #[test]
    fn chooses_nvenc_when_present() {
        let caps = nvenc_caps();
        assert_eq!(caps.h264_encoder, "h264_nvenc");
        assert_eq!(caps.hevc_encoder, "hevc_nvenc");
        assert_eq!(caps.hwaccel.as_deref(), Some("cuda"));
        assert!(caps.hw_encoders.contains(&"h264_nvenc".to_string()));
        assert!(!caps.hw_encoders.contains(&"libx264".to_string()));
    }

    #[test]
    fn falls_back_to_software() {
        let caps = software_caps();
        assert_eq!(caps.h264_encoder, "libx264");
        assert_eq!(caps.hwaccel, None);
        assert!(caps.hw_encoders.is_empty());
    }

    #[test]
    fn remux_copies_video_and_audio() {
        let args = remux_args("/media/movie.mkv", "h264", true, None);
        assert!(args.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "copy"]));
        assert!(!args.iter().any(|a| a == "aac"));
        assert!(args
            .windows(2)
            .any(|w| w == ["-hls_segment_type", "mpegts"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-hls_segment_filename", "seg_%05d.ts"]));
        assert_eq!(args.last().unwrap(), "master.m3u8");
    }

    #[test]
    fn remux_reencodes_unsupported_audio() {
        let args = remux_args("/media/movie.mkv", "h264", false, None);
        assert!(args.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "aac"]));
        assert!(args.windows(2).any(|w| w == ["-b:a", "192k"]));
    }

    #[test]
    fn remux_hevc_uses_fmp4() {
        let args = remux_args("/media/movie.mkv", "hevc", true, None);
        assert!(args.windows(2).any(|w| w == ["-hls_segment_type", "fmp4"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-hls_segment_filename", "seg_%05d.m4s"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-hls_fmp4_init_filename", "init.mp4"]));
    }

    #[test]
    fn remux_honours_start_position() {
        let args = remux_args("/media/movie.mkv", "h264", true, Some(90_500));
        let i = args.iter().position(|a| a == "-ss").unwrap();
        assert_eq!(args[i + 1], "90.500");
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert!(i < input, "-ss must precede -i for fast seeking");
    }

    #[tokio::test]
    async fn completed_hls_media_has_an_end_marker() {
        // Exercise ffmpeg output rather than only checking a command-line flag.
        if !tokio::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .await
            .map(|output| output.status.success())
            .unwrap_or(false)
        {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("sample.mkv");
        let output = tokio::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=64x64:rate=5",
                "-f",
                "lavfi",
                "-i",
                "sine=sample_rate=48000",
                "-t",
                "0.4",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:a",
                "aac",
            ])
            .arg(&input)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = tokio::process::Command::new("ffmpeg")
            .args(remux_args(input.to_str().unwrap(), "h264", true, None))
            .current_dir(temp.path())
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let playlist = tokio::fs::read_to_string(temp.path().join("master.m3u8"))
            .await
            .unwrap();
        assert!(playlist.contains("#EXT-X-ENDLIST"), "{playlist}");
    }

    #[test]
    fn nvenc_transcode_flags() {
        let caps = nvenc_caps();
        let args = transcode_args(
            &caps,
            "/media/movie.mkv",
            false,
            Some("scale=-2:1080"),
            Some(6_000_000),
            None,
        );
        assert!(args.windows(2).any(|w| w == ["-hwaccel", "cuda"]));
        assert!(args.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
        assert!(args.windows(2).any(|w| w == ["-preset", "p4"]));
        assert!(args.windows(2).any(|w| w == ["-cq", "23"]));
        assert!(args.windows(2).any(|w| w == ["-vf", "scale=-2:1080"]));
        assert!(args.windows(2).any(|w| w == ["-b:v", "6000000"]));
        assert!(args.windows(2).any(|w| w == ["-maxrate", "9000000"]));
        assert!(args.windows(2).any(|w| w == ["-bufsize", "12000000"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-hls_segment_type", "mpegts"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "aac"]));
        assert_eq!(args.last().unwrap(), "master.m3u8");
    }

    #[test]
    fn software_transcode_has_no_hwaccel() {
        let caps = software_caps();
        let args = transcode_args(&caps, "/media/movie.mkv", false, None, None, None);
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(args.windows(2).any(|w| w == ["-preset", "veryfast"]));
        assert!(args.windows(2).any(|w| w == ["-crf", "23"]));
        assert!(!args.iter().any(|a| a == "-hwaccel"));
    }

    #[test]
    fn hevc_transcode_uses_fmp4() {
        let caps = nvenc_caps();
        let args = transcode_args(&caps, "/media/movie.mkv", true, None, None, None);
        assert!(args.windows(2).any(|w| w == ["-c:v", "hevc_nvenc"]));
        assert!(args.windows(2).any(|w| w == ["-hls_segment_type", "fmp4"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["-hls_segment_filename", "seg_%05d.m4s"]));
    }

    #[test]
    fn scale_filter_variants() {
        assert_eq!(scale_filter(None, None), None);
        assert_eq!(scale_filter(Some(1920), None).unwrap(), "scale=1920:-2");
        assert_eq!(scale_filter(None, Some(1080)).unwrap(), "scale=-2:1080");
        assert!(scale_filter(Some(1920), Some(1080))
            .unwrap()
            .contains("force_original_aspect_ratio=decrease"));
    }
}
