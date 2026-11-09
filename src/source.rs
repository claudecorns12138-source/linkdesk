use crate::protocol::{self, VideoFrame};
#[cfg(any(windows, target_os = "macos"))]
use anyhow::Context;
use anyhow::{bail, Result};
use openh264::{
    encoder::Encoder,
    formats::{RgbSliceU8, YUVBuffer},
};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::{
    process::Child,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Extend,
    Mirror,
    Test,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Extend => "Extend desktop",
            Self::Mirror => "Mirror display",
            Self::Test => "Connection test",
        }
    }
}

pub struct SourceControl {
    pub child: Mutex<Option<Child>>,
}
impl Default for SourceControl {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
        }
    }
}
impl SourceControl {
    pub fn stop(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn run(
    mode: Mode,
    stop: Arc<AtomicBool>,
    control: Arc<SourceControl>,
    emit: impl FnMut(VideoFrame) -> Result<()>,
) -> Result<()> {
    if mode == Mode::Test {
        return pattern(stop, emit);
    }
    #[cfg(target_os = "macos")]
    {
        mac_capture(mode, stop, control, emit)
    }
    #[cfg(windows)]
    {
        let _ = control;
        if mode == Mode::Extend {
            bail!("Windows extended-display driver is not implemented yet. Use this PC as a receiver, or choose Mirror.")
        }
        windows_capture(stop, emit)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (stop, control, emit);
        bail!("Native capture is not implemented for this platform yet; receiver and connection test are available")
    }
}

fn encode_rgb(
    encoder: &mut Encoder,
    rgb: &[u8],
    width: usize,
    height: usize,
    sequence: u64,
) -> Result<VideoFrame> {
    if sequence.is_multiple_of(30) {
        encoder.force_intra_frame();
    }
    let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(rgb, (width, height)));
    let data = encoder.encode(&yuv)?.to_vec();
    Ok(VideoFrame {
        sequence,
        width: width as u32,
        height: height as u32,
        keyframe: protocol::contains_idr(&data),
        data,
    })
}

pub fn test_frame(sequence: u64, encoder: &mut Encoder) -> Result<VideoFrame> {
    let (w, h) = (640, 360);
    let mut rgb = vec![0; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            let block = ((x + sequence as usize * 5) / 80 + y / 90) % 2;
            let color = if block == 0 {
                [13, 28, 44]
            } else {
                [21, 68, 80]
            };
            rgb[i..i + 3].copy_from_slice(&color);
            if x.abs_diff((sequence as usize * 7) % w) < 6 || y.abs_diff(h / 2) < 2 {
                rgb[i..i + 3].copy_from_slice(&[71, 224, 190]);
            }
        }
    }
    encode_rgb(encoder, &rgb, w, h, sequence)
}

fn pattern(stop: Arc<AtomicBool>, mut emit: impl FnMut(VideoFrame) -> Result<()>) -> Result<()> {
    let mut encoder = Encoder::new()?;
    let mut seq = 0;
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let frame = test_frame(seq, &mut encoder)?;
        if !frame.data.is_empty() {
            emit(frame)?;
        }
        seq += 1;
        thread::sleep(Duration::from_millis(33).saturating_sub(started.elapsed()));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_capture(
    stop: Arc<AtomicBool>,
    mut emit: impl FnMut(VideoFrame) -> Result<()>,
) -> Result<()> {
    use scrap::{Capturer, Display};
    let mut capture =
        Capturer::new(Display::primary()?).context("cannot capture Windows display")?;
    let (native_w, native_h) = (capture.width(), capture.height());
    // A bounded 1080p software fallback; Mac uses VideoToolbox instead.
    let scale = (native_w as f64 / 1920.0)
        .max(native_h as f64 / 1080.0)
        .max(1.0);
    let (w, h) = (
        ((native_w as f64 / scale) as usize) & !1,
        ((native_h as f64 / scale) as usize) & !1,
    );
    let mut encoder = Encoder::new()?;
    let mut seq = 0;
    let mut rgb = vec![0; w * h * 3];
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        match capture.frame() {
            Ok(frame) => {
                let stride = frame.len() / native_h;
                for y in 0..h {
                    for x in 0..w {
                        let from = (y * native_h / h) * stride + (x * native_w / w) * 4;
                        let to = (y * w + x) * 3;
                        rgb[to..to + 3].copy_from_slice(&[
                            frame[from + 2],
                            frame[from + 1],
                            frame[from],
                        ]);
                    }
                }
                let frame = encode_rgb(&mut encoder, &rgb, w, h, seq)?;
                if !frame.data.is_empty() {
                    emit(frame)?;
                }
                seq += 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        thread::sleep(Duration::from_millis(66).saturating_sub(started.elapsed()));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn mac_capture(
    mode: Mode,
    stop: Arc<AtomicBool>,
    control: Arc<SourceControl>,
    mut emit: impl FnMut(VideoFrame) -> Result<()>,
) -> Result<()> {
    let exe = std::env::current_exe()?;
    let helper = exe
        .parent()
        .context("missing executable folder")?
        .join("linkdesk-mac-helper");
    let mut child = Command::new(helper)
        .arg(if mode == Mode::Extend {
            "extend"
        } else {
            "mirror"
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Mac helper missing; run scripts/build-macos.sh")?;
    let mut stdout = child.stdout.take().context("missing helper output")?;
    *control.child.lock().unwrap() = Some(child);
    let mut seq = 0;
    while !stop.load(Ordering::Relaxed) {
        let (kind, data) = protocol::read_record(&mut stdout)?;
        match kind {
            protocol::JSON => {
                let v: serde_json::Value = serde_json::from_slice(&data)?;
                if let Some(error) = v.get("error").and_then(|v| v.as_str()) {
                    bail!("Mac capture: {error}")
                }
            }
            protocol::VIDEO => {
                let mut frame = protocol::decode_video(&data)?;
                frame.sequence = seq;
                seq += 1;
                emit(frame)?;
            }
            protocol::ERROR => bail!("Mac capture: {}", String::from_utf8_lossy(&data)),
            _ => bail!("invalid capture helper output"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use openh264::{decoder::Decoder, formats::YUVSource};
    #[test]
    fn native_h264_pattern_round_trip() {
        let mut enc = Encoder::new().unwrap();
        let mut dec = Decoder::new().unwrap();
        let frame = test_frame(0, &mut enc).unwrap();
        assert!(frame.keyframe);
        let decoded = dec.decode(&frame.data).unwrap().unwrap();
        assert_eq!(decoded.dimensions(), (640, 360));
        let mut rgb = vec![0; 640 * 360 * 3];
        decoded.write_rgb8(&mut rgb);
        assert!(rgb.iter().any(|&v| v > 100));
    }
}
