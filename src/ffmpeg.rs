//! Encoder, format and filter names from the local ffmpeg build, listed in the background.
use crate::{
    names::{Names, Ref},
    sources::Env,
};
use std::time::Duration;
const MAX_NAMES: usize = 5_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Encoder,
    VideoEncoder,
    AudioEncoder,
    SubtitleEncoder,
    Format,
    VideoFilter,
    AudioFilter,
}
/// The build's lists only change when ffmpeg is upgraded.
const TTL: Duration = Duration::from_secs(600);
pub fn complete(env: &Env, kind: Kind) -> Vec<Ref> {
    let list = match kind {
        Kind::Format => "-formats",
        Kind::VideoFilter | Kind::AudioFilter => "-filters",
        Kind::Encoder | Kind::VideoEncoder | Kind::AudioEncoder | Kind::SubtitleEncoder => {
            "-encoders"
        }
    };
    let lines = env
        .runs
        .lines(env.path, &["ffmpeg", "-hide_banner", list], TTL);
    match kind {
        Kind::Format => formats(&lines),
        Kind::VideoFilter => filters(&lines, "V->V"),
        Kind::AudioFilter => filters(&lines, "A->A"),
        Kind::Encoder => encoders(&lines, ""),
        Kind::VideoEncoder => encoders(&lines, "V"),
        Kind::AudioEncoder => encoders(&lines, "A"),
        Kind::SubtitleEncoder => encoders(&lines, "S"),
    }
}
/// Lines after the `---` separator that ends the legend.
fn body(lines: &[String]) -> (usize, &[String]) {
    match lines.iter().position(|l| l.trim().starts_with("--")) {
        Some(i) => (lines[i].trim().len(), &lines[i + 1..]),
        None => (0, &[]),
    }
}
/// ` V....D libx264   description`; `copy` keeps streams as they are and is not an encoder.
fn encoders(lines: &[String], kind: &str) -> Vec<Ref> {
    let mut out = Names::new(MAX_NAMES);
    out.push("copy", "Copy the stream without re-encoding");
    for l in body(lines).1 {
        let mut words = l.trim_start().splitn(3, ' ');
        if let (Some(flags), Some(name)) = (words.next(), words.next())
            && flags.starts_with(kind)
        {
            out.push(name, words.next().unwrap_or(""));
        }
    }
    out.finish()
}
/// ` DE  matroska,webm  description`: the flag column may contain spaces, so it is cut by width.
fn formats(lines: &[String]) -> Vec<Ref> {
    let (width, body) = body(lines);
    let mut out = Names::new(MAX_NAMES);
    for l in body {
        let Some(rest) = l.get(1 + width..) else {
            continue;
        };
        let rest = rest.trim_start();
        let (names, description) = rest.split_once(' ').unwrap_or((rest, ""));
        for name in names.split(',') {
            out.push(name, description);
        }
    }
    out.finish()
}
// ponytail: only single-input, single-output filters of one media type; -filter_complex stays free text.
/// ` T. scale   V->V   description`.
fn filters(lines: &[String], io: &str) -> Vec<Ref> {
    let mut out = Names::new(MAX_NAMES);
    for l in body(lines).1 {
        let mut words = l.split_whitespace();
        if let (Some(_), Some(name), Some(kind)) = (words.next(), words.next(), words.next())
            && kind == io
            && let Some((_, description)) = l.split_once(kind)
        {
            out.push(name, description);
        }
    }
    out.finish()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_owned).collect()
    }
    fn names(refs: &[Ref]) -> Vec<&str> {
        refs.iter().map(|r| r.name.as_str()).collect()
    }
    #[test]
    fn parses_ffmpeg_lists() {
        let enc = lines(
            "Encoders:\n V..... = Video\n A..... = Audio\n ------\n V....D libx264              libx264 H.264 (codec h264)\n A....D aac                  AAC (Advanced Audio Coding)\n S..... mov_text             3GPP Timed Text subtitle\n",
        );
        assert_eq!(names(&encoders(&enc, "V")), ["copy", "libx264"]);
        assert_eq!(
            names(&encoders(&enc, "")),
            ["aac", "copy", "libx264", "mov_text"]
        );
        assert_eq!(
            encoders(&enc, "A")[0].description,
            "AAC (Advanced Audio Coding)"
        );
        let fmt = lines(
            "Formats:\n D.. = Demuxing supported\n ---\n D   3dostr          3DO STR\n  E  matroska        Matroska\n D   matroska,webm   Matroska / WebM\n DEd avfoundation    AVFoundation input device\n",
        );
        assert_eq!(
            names(&formats(&fmt)),
            ["3dostr", "avfoundation", "matroska", "webm"]
        );
        assert_eq!(formats(&fmt)[2].description, "Matroska");
        let fil = lines(
            "Filters:\n  T.. = Timeline support\n  ------\n .. scale             V->V       Scale the input video.\n T. volume            A->A       Change input volume.\n .. overlay           VV->V      Overlay a video.\n .. anullsrc          |->A       Null audio source.\n",
        );
        assert_eq!(names(&filters(&fil, "V->V")), ["scale"]);
        assert_eq!(filters(&fil, "A->A")[0].description, "Change input volume.");
        assert!(encoders(&[], "V").len() == 1 && formats(&[]).is_empty());
    }
}
