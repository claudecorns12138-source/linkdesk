//! Plan an adjacent Mac display without moving any existing screen.
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

pub const EXTENSION_WIDTH: i64 = 1920;
pub const EXTENSION_HEIGHT: i64 = 1080;
const MIN_PASSAGE: i64 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}
impl Rect {
    pub fn right(self) -> i64 {
        self.x + self.width
    }
    pub fn bottom(self) -> i64 {
        self.y + self.height
    }
    pub fn valid(self) -> bool {
        (1..=32768).contains(&self.width)
            && (1..=32768).contains(&self.height)
            && [
                self.x,
                self.y,
                self.x.saturating_add(self.width),
                self.y.saturating_add(self.height),
            ]
            .iter()
            .all(|n| i32::try_from(*n).is_ok())
    }
    pub fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub id: u32,
    pub rect: Rect,
    pub main: bool,
    pub active: bool,
    pub mirrored_to: u32,
}
impl Display {
    pub fn label(&self, index: usize) -> String {
        format!(
            "{} · {} × {}",
            if self.main {
                "Mac 主屏".into()
            } else {
                format!("现有扩展屏 {}", index + 1)
            },
            self.rect.width,
            self.rect.height
        )
    }
    pub fn can_anchor(&self) -> bool {
        self.active && self.mirrored_to == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Auto,
    Right,
    Left,
    Above,
    Below,
}
impl Side {
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::Right,
        Self::Left,
        Self::Above,
        Self::Below,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动选择空闲边缘",
            Self::Right => "右侧",
            Self::Left => "左侧",
            Self::Above => "上方",
            Self::Below => "下方",
        }
    }
    pub fn argument(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Right => "right",
            Self::Left => "left",
            Self::Above => "above",
            Self::Below => "below",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ExtensionOptions {
    pub anchor: Option<u32>,
    pub side: Side,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub anchor_id: u32,
    pub side: Side,
    pub rect: Rect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveLayout {
    pub display_id: u32,
    pub placement: Placement,
    pub existing: Vec<Display>,
    pub existing_preserved: bool,
}

pub fn validate_displays(displays: &[Display]) -> Result<()> {
    ensure!(
        !displays.is_empty() && displays.len() <= 64,
        "无法安全读取现有屏幕列表。请刷新屏幕排列。"
    );
    let mut ids = std::collections::BTreeSet::new();
    for display in displays {
        ensure!(
            display.id != 0 && ids.insert(display.id) && display.rect.valid(),
            "系统返回了无效的屏幕排列。"
        );
    }
    ensure!(
        displays.iter().filter(|d| d.main).count() == 1,
        "未找到唯一的 Mac 主屏。"
    );
    Ok(())
}

pub fn adjacent(rect: Rect, anchor: Rect, side: Side) -> bool {
    let vertical = rect.bottom().min(anchor.bottom()) - rect.y.max(anchor.y);
    let horizontal = rect.right().min(anchor.right()) - rect.x.max(anchor.x);
    match side {
        Side::Right => rect.x == anchor.right() && vertical >= MIN_PASSAGE,
        Side::Left => rect.right() == anchor.x && vertical >= MIN_PASSAGE,
        Side::Above => rect.bottom() == anchor.y && horizontal >= MIN_PASSAGE,
        Side::Below => rect.y == anchor.bottom() && horizontal >= MIN_PASSAGE,
        Side::Auto => false,
    }
}

pub fn validate_placement(displays: &[Display], placement: Placement) -> Result<()> {
    validate_displays(displays)?;
    let anchor = displays
        .iter()
        .find(|d| d.id == placement.anchor_id && d.can_anchor())
        .context("选中的相邻屏幕已离线或处于镜像状态。请刷新屏幕排列。");
    let anchor = anchor?;
    ensure!(
        placement.rect.valid()
            && placement.rect.width == EXTENSION_WIDTH
            && placement.rect.height == EXTENSION_HEIGHT,
        "新增屏幕的尺寸无效。"
    );
    ensure!(
        adjacent(placement.rect, anchor.rect, placement.side),
        "新增屏幕没有与选中的屏幕相接。"
    );
    ensure!(
        displays
            .iter()
            .all(|d| !d.can_anchor() || !placement.rect.intersects(d.rect)),
        "该位置被现有屏幕占用。请选择另一侧。"
    );
    Ok(())
}

pub fn plan(displays: &[Display], options: ExtensionOptions) -> Result<Placement> {
    validate_displays(displays)?;
    let mut anchors: Vec<_> = displays
        .iter()
        .filter(|d| d.can_anchor() && options.anchor.is_none_or(|id| id == d.id))
        .collect();
    // Existing independent extensions (including Sidecar) are preferred so a
    // Mac window can cross directly from that screen to the new receiver.
    anchors.sort_by_key(|d| (d.main, d.id));
    ensure!(
        !anchors.is_empty(),
        "选中的相邻屏幕已离线或处于镜像状态。请刷新屏幕排列。"
    );
    let sides: &[Side] = if options.side == Side::Auto {
        &[Side::Right, Side::Left, Side::Below, Side::Above]
    } else {
        std::slice::from_ref(&options.side)
    };
    for anchor in anchors {
        for &side in sides {
            let vertical = matches!(side, Side::Right | Side::Left);
            let (start, length, extent) = if vertical {
                (anchor.rect.y, anchor.rect.height, EXTENSION_HEIGHT)
            } else {
                (anchor.rect.x, anchor.rect.width, EXTENSION_WIDTH)
            };
            let center = start + (length - extent) / 2;
            let mut offsets = vec![center, start, start + length - extent];
            for obstacle in displays.iter().filter(|d| d.can_anchor()) {
                let (min, max) = if vertical {
                    (obstacle.rect.y, obstacle.rect.bottom())
                } else {
                    (obstacle.rect.x, obstacle.rect.right())
                };
                offsets.extend([min - extent, max]);
            }
            offsets.sort_unstable();
            offsets.dedup();
            offsets.sort_by_key(|offset| (offset.abs_diff(center), *offset));
            for offset in offsets {
                let (x, y) = match side {
                    Side::Right => (anchor.rect.right(), offset),
                    Side::Left => (anchor.rect.x - EXTENSION_WIDTH, offset),
                    Side::Above => (offset, anchor.rect.y - EXTENSION_HEIGHT),
                    Side::Below => (offset, anchor.rect.bottom()),
                    Side::Auto => unreachable!(),
                };
                let result = Placement {
                    anchor_id: anchor.id,
                    side,
                    rect: Rect {
                        x,
                        y,
                        width: EXTENSION_WIDTH,
                        height: EXTENSION_HEIGHT,
                    },
                };
                if validate_placement(displays, result).is_ok() {
                    return Ok(result);
                }
            }
        }
    }
    bail!("该侧没有安全相接的空位；现有屏幕不会被移动。请选择另一侧或其他相邻屏幕。")
}

#[cfg(target_os = "macos")]
pub fn helper_path() -> Result<std::path::PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .context("missing executable folder")?
        .join("linkdesk-mac-helper"))
}

pub fn read_displays() -> Result<Vec<Display>> {
    #[cfg(target_os = "macos")]
    {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        let mut child = Command::new(helper_path()?)
            .arg("list-displays")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Mac 采集组件缺失，请使用完整的 LinkDesk.app。")?;
        let deadline = Instant::now() + Duration::from_secs(4);
        while child.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("读取屏幕排列超时，请刷新后重试。");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let status = child.wait()?;
        let mut output = child.stdout.take().context("missing helper output")?;
        // The helper emits one small bounded control record, never a desktop image.
        let (kind, data) = crate::protocol::read_record_limit(&mut output, 4097)?;
        if kind == crate::protocol::ERROR {
            bail!("Mac 屏幕排列：{}", String::from_utf8_lossy(&data));
        }
        ensure!(
            kind == crate::protocol::JSON,
            "Mac 采集组件返回了无效的屏幕列表。"
        );
        let value: serde_json::Value = serde_json::from_slice(&data)?;
        ensure!(status.success(), "读取屏幕排列失败。");
        let displays: Vec<Display> = serde_json::from_value(value["displays"].clone())?;
        validate_displays(&displays)?;
        Ok(displays)
    }
    #[cfg(not(target_os = "macos"))]
    {
        bail!("屏幕排列目前由 Mac 主机管理。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screen(id: u32, x: i64, y: i64, width: i64, height: i64) -> Display {
        Display {
            id,
            rect: Rect {
                x,
                y,
                width,
                height,
            },
            main: id == 1,
            active: true,
            mirrored_to: 0,
        }
    }
    #[test]
    fn sidecar_above_main_has_a_direct_shared_edge_without_moving_either() {
        let displays = vec![
            screen(1, 0, 0, 1728, 1117),
            screen(2, 300, -1024, 1366, 1024),
        ];
        let original = displays.clone();
        let p = plan(&displays, ExtensionOptions::default()).unwrap();
        assert_eq!(p.anchor_id, 2);
        assert_eq!(p.side, Side::Right);
        assert!(adjacent(p.rect, displays[1].rect, p.side));
        assert!(!p.rect.intersects(displays[0].rect));
        assert_eq!(original, displays);
    }
    #[test]
    fn sidecar_left_of_main_can_use_its_left_edge_instead_of_overlapping_main() {
        let displays = vec![screen(1, 0, 0, 1728, 1117), screen(2, -1366, 0, 1366, 1024)];
        let p = plan(
            &displays,
            ExtensionOptions {
                anchor: Some(2),
                side: Side::Left,
            },
        )
        .unwrap();
        assert_eq!(p.rect.right(), -1366);
        assert!(validate_placement(&displays, p).is_ok());
    }
    #[test]
    fn occupied_edge_fails_instead_of_rearranging_the_other_screens() {
        let displays = vec![screen(1, 0, 0, 1728, 1117), screen(2, 1728, 0, 1920, 1080)];
        assert!(plan(
            &displays,
            ExtensionOptions {
                anchor: Some(1),
                side: Side::Right
            }
        )
        .is_err());
    }
    #[test]
    fn negative_coordinates_and_every_side_keep_a_nonzero_passage() {
        let displays = vec![screen(1, -2000, -1200, 1728, 1117)];
        for side in [Side::Right, Side::Left, Side::Above, Side::Below] {
            let p = plan(
                &displays,
                ExtensionOptions {
                    anchor: Some(1),
                    side,
                },
            )
            .unwrap();
            assert!(validate_placement(&displays, p).is_ok());
        }
    }
    #[test]
    fn stale_mirrored_and_invalid_screens_are_rejected() {
        let mut displays = vec![screen(1, 0, 0, 1728, 1117), screen(2, -1366, 0, 1366, 1024)];
        displays[1].mirrored_to = 1;
        assert!(plan(
            &displays,
            ExtensionOptions {
                anchor: Some(2),
                side: Side::Auto
            }
        )
        .is_err());
        assert!(plan(
            &displays,
            ExtensionOptions {
                anchor: Some(99),
                side: Side::Auto
            }
        )
        .is_err());
        displays[1].rect.x = i64::MAX;
        assert!(plan(&displays, ExtensionOptions::default()).is_err());
    }
    #[test]
    fn generated_arrangements_never_overlap_or_change_existing_displays() {
        for x in [-1920, 0, 400, 1920] {
            for y in [-1400, -1080, 0, 1100, 2200] {
                let displays = vec![screen(1, 0, 0, 1728, 1117), screen(2, x, y, 1366, 1024)];
                let before = displays.clone();
                for side in Side::ALL {
                    if let Ok(p) = plan(
                        &displays,
                        ExtensionOptions {
                            anchor: Some(2),
                            side,
                        },
                    ) {
                        validate_placement(&displays, p).unwrap();
                        assert!(displays.iter().all(|d| !d.rect.intersects(p.rect)));
                    }
                    assert_eq!(displays, before);
                }
            }
        }
    }
}
