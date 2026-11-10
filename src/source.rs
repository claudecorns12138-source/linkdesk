use crate::{
    desktop::{ExtensionOptions, LiveLayout},
    protocol::{self, VideoFrame},
};
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
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
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

pub use crate::managed_child::SourceControl;

pub fn run(
    mode: Mode,
    stop: Arc<AtomicBool>,
    control: Arc<SourceControl>,
    emit: impl FnMut(VideoFrame) -> Result<()>,
) -> Result<()> {
    run_with_layout(
        mode,
        ExtensionOptions::default(),
        stop,
        control,
        |_| {},
        emit,
    )
}

pub fn run_with_layout(
    mode: Mode,
    options: ExtensionOptions,
    stop: Arc<AtomicBool>,
    control: Arc<SourceControl>,
    ready: impl FnMut(LiveLayout),
    emit: impl FnMut(VideoFrame) -> Result<()>,
) -> Result<()> {
    if mode == Mode::Test {
        return pattern(stop, emit);
    }
    #[cfg(target_os = "macos")]
    {
        mac_capture(mode, options, stop, control, ready, emit)
    }
    #[cfg(windows)]
    {
        let _ = (control, options, ready);
        if mode == Mode::Extend {
            bail!("Windows extended-display driver is not implemented yet. Use this PC as a receiver, or choose Mirror.")
        }
        windows_capture(stop, emit)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (stop, control, emit, options, ready);
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
    options: ExtensionOptions,
    stop: Arc<AtomicBool>,
    control: Arc<SourceControl>,
    mut ready: impl FnMut(LiveLayout),
    mut emit: impl FnMut(VideoFrame) -> Result<()>,
) -> Result<()> {
    let planned = if mode == Mode::Extend {
        Some(crate::desktop::plan(
            &crate::desktop::read_displays()?,
            options,
        )?)
    } else {
        None
    };
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut command = Command::new(crate::desktop::helper_path()?);
    if let Some(p) = planned {
        command
            .arg("extend")
            .arg("--anchor")
            .arg(p.anchor_id.to_string())
            .arg("--side")
            .arg(p.side.argument())
            .arg("--x")
            .arg(p.rect.x.to_string())
            .arg("--y")
            .arg(p.rect.y.to_string());
    } else {
        command.arg("mirror");
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Mac helper missing; run scripts/build-macos.sh")?;
    let mut stdout = child.stdout.take().context("missing helper output")?;
    if stop.load(Ordering::Relaxed) {
        control.stop();
    }
    // The lease also stops/reaps the helper on early return or unwinding.
    // A permanently stopped control rejects and reaps a late subprocess.
    let Some(_session) = control.attach(child) else {
        return Ok(());
    };
    let mut seq = 0;
    while !stop.load(Ordering::Relaxed) {
        let (kind, data) = protocol::read_record(&mut stdout)?;
        match kind {
            protocol::JSON => {
                let v: serde_json::Value = serde_json::from_slice(&data)?;
                if let Some(error) = v.get("error").and_then(|v| v.as_str()) {
                    bail!("Mac capture: {error}")
                }
                if let Some(planned) = planned {
                    let layout: LiveLayout = serde_json::from_value(v["layout"].clone())
                        .context("Mac helper did not confirm preservation of existing displays")?;
                    anyhow::ensure!(
                        layout.existing_preserved
                            && layout.placement == planned
                            && layout.display_id != 0
                            && layout.existing.iter().all(|d| d.id != layout.display_id),
                        "Mac helper did not confirm the requested independent layout"
                    );
                    crate::desktop::validate_placement(&layout.existing, layout.placement)?;
                    ready(layout);
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
