use eframe::egui::{self, Color32, FontId, RichText, Vec2};
use linkdesk::desktop::{
    self, Display, ExtensionOptions, LiveLayout, Placement, Rect, RouteState, Side,
};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

const ACCENT: Color32 = Color32::from_rgb(95, 231, 202);
const MUTED: Color32 = Color32::from_rgb(143, 166, 179);
const BLUE: Color32 = Color32::from_rgb(126, 192, 245);

#[derive(Default)]
pub struct LayoutControls {
    displays: Vec<Display>,
    options: ExtensionOptions,
    pending: Option<Receiver<Result<Vec<Display>, String>>>,
    error: Option<String>,
    live: Option<LiveLayout>,
    preview: bool,
    query_live: bool,
    current: Option<(Vec<Display>, Instant)>,
    preview_state: u8,
}

impl LayoutControls {
    pub fn preview() -> Self {
        Self {
            displays: vec![
                Display {
                    id: 1,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        width: 1728,
                        height: 1117,
                    },
                    main: true,
                    active: true,
                    mirrored_to: 0,
                },
                Display {
                    id: 2,
                    rect: Rect {
                        x: 300,
                        y: -1024,
                        width: 1366,
                        height: 1024,
                    },
                    main: false,
                    active: true,
                    mirrored_to: 0,
                },
            ],
            options: ExtensionOptions {
                anchor: Some(2),
                side: Side::Right,
            },
            preview: true,
            ..Default::default()
        }
    }
    pub fn options(&self) -> ExtensionOptions {
        self.options
    }
    pub fn ready(&mut self, layout: LiveLayout) {
        self.pending = None;
        self.error = None;
        self.current = None;
        self.displays = layout.existing.clone();
        self.live = Some(layout);
    }
    pub fn clear_live(&mut self) {
        self.live = None;
        self.current = None;
        self.pending = None;
        self.error = None;
        if !self.preview {
            self.displays.clear();
        }
    }
    fn refresh(&mut self, ctx: &egui::Context) {
        if self.preview {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.query_live = self.live.is_some();
        self.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(desktop::read_displays().map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
    }
    fn poll(&mut self) {
        let result = self.pending.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(value) => Some(value),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("屏幕读取任务已结束，请重试。".into()))
            }
        });
        if let Some(result) = result {
            self.pending = None;
            match result {
                Ok(displays) => {
                    if self.query_live {
                        self.current = Some((displays, Instant::now()));
                    } else {
                        self.displays = displays;
                    }
                    self.error = None;
                }
                Err(error) => {
                    if self.query_live {
                        self.current = None;
                    } else {
                        self.displays.clear();
                    }
                    self.error = Some(error);
                }
            }
        }
    }
    pub fn show_setup(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.poll();
        if self.displays.is_empty() && self.pending.is_none() && self.error.is_none() {
            self.refresh(ctx);
        }
        ui.add_space(14.);
        ui.separator();
        ui.add_space(10.);
        ui.label(
            RichText::new("接上现有随航，直接跨屏拖动")
                .strong()
                .size(17.),
        );
        ui.label(
            RichText::new("保留 iPad 的苹果随航；只为接收端新增一块 Mac 扩展屏。")
                .color(MUTED)
                .size(13.),
        );
        if self.preview {
            ui.label(
                RichText::new("布局演示 · 以下屏幕为虚构数据，不代表 Mac 实机状态。")
                    .color(BLUE)
                    .size(12.),
            );
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.pending.is_none() && !self.preview,
                    egui::Button::new("刷新屏幕排列"),
                )
                .clicked()
            {
                self.refresh(ctx);
            }
            if self.pending.is_some() {
                ui.spinner();
                ui.label("正在读取 Mac 屏幕…");
            }
        });
        if let Some(error) = &self.error {
            ui.label(
                RichText::new(error)
                    .color(Color32::from_rgb(255, 161, 149))
                    .size(12.),
            );
        }
        if self.displays.is_empty() {
            return;
        }
        ui.add_space(8.);
        egui::Grid::new("extension-placement")
            .spacing([16., 8.])
            .show(ui, |ui| {
                ui.label("相邻屏幕");
                let selected_label = self
                    .options
                    .anchor
                    .and_then(|id| self.displays.iter().position(|d| d.id == id))
                    .map(|i| label(&self.displays, i))
                    .unwrap_or_else(|| {
                        if self.options.anchor.is_some() {
                            "所选屏幕已离线，请重新选择".into()
                        } else {
                            "自动，优先现有扩展屏".into()
                        }
                    });
                egui::ComboBox::from_id_salt("extension-anchor")
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.options.anchor, None, "自动，优先现有扩展屏");
                        for (i, display) in self
                            .displays
                            .iter()
                            .enumerate()
                            .filter(|(_, d)| d.can_anchor())
                        {
                            ui.selectable_value(
                                &mut self.options.anchor,
                                Some(display.id),
                                label(&self.displays, i),
                            );
                        }
                    });
                ui.end_row();
                ui.label("接收屏位置");
                egui::ComboBox::from_id_salt("extension-side")
                    .selected_text(self.options.side.label())
                    .show_ui(ui, |ui| {
                        for side in Side::ALL {
                            ui.selectable_value(&mut self.options.side, side, side.label());
                        }
                    });
                ui.end_row();
            });
        match desktop::plan(&self.displays, self.options) {
            Ok(plan) => {
                ui.add_space(10.);
                if let Some(id) = diagram(ui, &self.displays, Some(plan), true) {
                    self.options.anchor = Some(id);
                }
                ui.label(
                    RichText::new("蓝色：相邻屏幕　绿色：新增接收屏　亮线：可拖动边缘")
                        .color(MUTED)
                        .size(12.),
                );
                ui.label(
                    RichText::new(
                        "从随航屏上的 Mac 窗口标题栏，拖过亮线到接收屏；反向同样可拖回。",
                    )
                    .size(13.),
                );
            }
            Err(error) => {
                if let Some(id) = diagram(ui, &self.displays, None, true) {
                    self.options.anchor = Some(id);
                }
                ui.label(
                    RichText::new(error.to_string())
                        .color(Color32::from_rgb(255, 161, 149))
                        .size(13.),
                );
            }
        }
        ui.label(
            RichText::new(
                "现有屏幕的位置、分辨率和镜像关系由系统管理。没有空位时会停止，而不会移动随航。",
            )
            .color(MUTED)
            .size(12.),
        );
    }
    pub fn show_live(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.poll();
        if let Some(layout) = self.live.clone() {
            ui.add_space(16.);
            ui.label(
                RichText::new("启动时已检查：现有屏幕状态保持一致")
                    .strong()
                    .color(ACCENT),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.pending.is_none() && !self.preview,
                        egui::Button::new("核对当前排列（只读取）"),
                    )
                    .clicked()
                {
                    self.refresh(ctx);
                }
                if self.pending.is_some() {
                    ui.spinner();
                    ui.label("正在核对…");
                }
            });
            if let Some(error) = &self.error {
                ui.label(
                    RichText::new(error)
                        .color(Color32::from_rgb(255, 161, 149))
                        .size(13.),
                );
            }
            if let Some((current, at)) = &self.current {
                match desktop::check_current(&layout, current) {
                    Ok(check) => {
                        let (text,healthy)=match check.state {
                            RouteState::Connected(p)=>(format!("当前通路相接 · {}，可沿亮线拖动 Mac 窗口",p.side.label()),true),
                            RouteState::AnchorUnavailable=>("选定的相邻屏幕已离线或处于镜像状态。请确认随航状态后再核对。".into(),false),
                            RouteState::ReceiverMissing=>("LinkDesk 屏已不在当前列表中。可结束共享后重新建立扩展屏。".into(),false),
                            RouteState::ReceiverUnavailable=>("接收屏已离线或处于镜像状态，当前不是独立扩展屏。".into(),false),
                            RouteState::Overlapping=>("当前独立屏幕区域重叠，无法确认通路。请等待系统排列完成后重试。".into(),false),
                            RouteState::NoSharedEdge=>("两块屏当前没有可穿过的公共边缘。可在系统中调整 LinkDesk 屏，或结束共享重新规划。".into(),false),
                        };
                        ui.label(RichText::new(text).strong().color(if healthy {
                            ACCENT
                        } else {
                            Color32::from_rgb(255, 161, 149)
                        }));
                        if check.existing_changed {
                            ui.label(
                                RichText::new(
                                    "检测到现有屏幕状态变化；本次只核对，未修改任何屏幕。",
                                )
                                .color(BLUE)
                                .size(12.),
                            );
                        }
                        let base: Vec<_> = current
                            .iter()
                            .filter(|d| d.id != layout.display_id)
                            .cloned()
                            .collect();
                        let side = match check.state {
                            RouteState::Connected(p) => p.side,
                            _ => Side::Auto,
                        };
                        let view = check.receiver_rect.map(|rect| Placement {
                            anchor_id: layout.placement.anchor_id,
                            side,
                            rect,
                        });
                        diagram(ui, &base, view, false);
                        ui.label(
                            RichText::new(format!(
                                "最近一次只读核对：{} 秒前；这是读取时的排列。",
                                at.elapsed().as_secs()
                            ))
                            .color(MUTED)
                            .size(12.),
                        );
                    }
                    Err(error) => {
                        ui.label(
                            RichText::new(error.to_string())
                                .color(Color32::from_rgb(255, 161, 149)),
                        );
                    }
                }
            } else {
                diagram(ui, &layout.existing, Some(layout.placement), false);
                ui.label(RichText::new("下图仍是启动时排列；当前通路尚未核对。移动或重连随航后，请点击上方只读核对。").color(MUTED).size(13.));
            }
            ui.label(
                RichText::new(
                    "使用 Mac 的键鼠拖动窗口；核对不会移动屏幕、重连随航或改变镜像关系。",
                )
                .color(MUTED)
                .size(12.),
            );
        } else {
            ui.add_space(12.);
            ui.label(
                RichText::new("等待 Mac 确认独立扩展及现有屏幕保护检查…")
                    .color(MUTED)
                    .size(13.),
            );
        }
    }

    pub fn show_preview(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let before = self.preview_state;
        egui::ComboBox::from_id_salt("layout-scenario")
            .selected_text(preview_label(self.preview_state))
            .show_ui(ui, |ui| {
                for n in 0..6 {
                    ui.selectable_value(&mut self.preview_state, n, preview_label(n));
                }
            });
        if before != self.preview_state {
            let scenario = self.preview_state;
            *self = Self::preview();
            self.preview_state = scenario;
            if scenario != 0 {
                let placement = desktop::plan(&self.displays, self.options).unwrap();
                let layout = LiveLayout {
                    display_id: 3,
                    placement,
                    existing: self.displays.clone(),
                    existing_preserved: true,
                };
                let mut current = self.displays.clone();
                current.push(Display {
                    id: 3,
                    rect: placement.rect,
                    main: false,
                    active: true,
                    mirrored_to: 0,
                });
                match scenario {
                    2 => current[1].rect.x = -2000,
                    3 => current.retain(|d| d.id != 2),
                    4 => current[2].mirrored_to = 1,
                    5 => {
                        current[2].rect = Rect {
                            x: 300,
                            y: -2104,
                            width: 1920,
                            height: 1080,
                        }
                    }
                    _ => {}
                }
                self.live = Some(layout);
                self.current = Some((current, Instant::now()));
            }
        }
        ui.label(
            RichText::new("虚构状态演示，不读取真实设备，不创建扩展屏。")
                .color(BLUE)
                .size(12.),
        );
        if self.preview_state == 0 {
            self.show_setup(ui, ctx);
        } else {
            self.show_live(ui, ctx);
        }
    }
}

fn preview_label(n: u8) -> &'static str {
    match n {
        0 => "规划一块相邻扩展屏",
        1 => "运行中：当前通路有效",
        2 => "运行中：随航位置已变化",
        3 => "运行中：相邻屏幕已离线",
        4 => "运行中：接收屏已改为镜像",
        _ => "运行中：用户改到上方，通路有效",
    }
}

fn label(displays: &[Display], index: usize) -> String {
    let ordinal = displays[..index].iter().filter(|d| !d.main).count();
    displays[index].label(ordinal)
}

fn diagram(
    ui: &mut egui::Ui,
    displays: &[Display],
    plan: Option<Placement>,
    interactive: bool,
) -> Option<u32> {
    let mut rects: Vec<_> = displays
        .iter()
        .filter(|d| d.active)
        .map(|d| d.rect)
        .collect();
    if let Some(p) = plan {
        rects.push(p.rect);
    }
    if rects.is_empty() {
        return None;
    }
    let min_x = rects.iter().map(|r| r.x).min().unwrap() as f32;
    let min_y = rects.iter().map(|r| r.y).min().unwrap() as f32;
    let max_x = rects.iter().map(|r| r.right()).max().unwrap() as f32;
    let max_y = rects.iter().map(|r| r.bottom()).max().unwrap() as f32;
    let (area, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(100.), 240.),
        egui::Sense::hover(),
    );
    let scale = ((area.width() - 28.) / (max_x - min_x).max(1.))
        .min((area.height() - 28.) / (max_y - min_y).max(1.));
    let origin = area.center() - Vec2::new(max_x - min_x, max_y - min_y) * scale / 2.;
    let mapped = |r: Rect| {
        egui::Rect::from_min_size(
            origin + Vec2::new(r.x as f32 - min_x, r.y as f32 - min_y) * scale,
            Vec2::new(r.width as f32, r.height as f32) * scale,
        )
    };
    ui.painter()
        .rect_filled(area, 8, Color32::from_rgb(9, 17, 23));
    let mut clicked = None;
    for (i, display) in displays.iter().enumerate().filter(|(_, d)| d.active) {
        let r = mapped(display.rect);
        let selected = plan.is_some_and(|p| p.anchor_id == display.id);
        let color = if selected { BLUE } else { MUTED };
        ui.painter().rect_filled(
            r.shrink(1.),
            4,
            if selected {
                Color32::from_rgb(28, 53, 73)
            } else {
                Color32::from_rgb(26, 39, 48)
            },
        );
        ui.painter().rect_stroke(
            r.shrink(1.),
            4,
            egui::Stroke::new(if selected { 2_f32 } else { 1_f32 }, color),
            egui::StrokeKind::Inside,
        );
        let title = if display.main {
            "Mac 主屏".into()
        } else {
            format!(
                "现有扩展屏 {}",
                displays[..i].iter().filter(|d| !d.main).count() + 1
            )
        };
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            title,
            FontId::proportional(12.),
            color,
        );
        if interactive
            && display.can_anchor()
            && ui
                .interact(
                    r,
                    ui.id().with(("anchor", display.id)),
                    egui::Sense::click(),
                )
                .on_hover_text(label(displays, i))
                .clicked()
        {
            clicked = Some(display.id);
        }
    }
    if let Some(p) = plan {
        let r = mapped(p.rect);
        let receiver_color = if p.side == Side::Auto {
            Color32::from_rgb(255, 161, 149)
        } else {
            ACCENT
        };
        ui.painter()
            .rect_filled(r.shrink(1.), 4, Color32::from_rgb(20, 60, 55));
        ui.painter().rect_stroke(
            r.shrink(1.),
            4,
            egui::Stroke::new(2_f32, receiver_color),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            "LinkDesk 接收屏",
            FontId::proportional(12.),
            receiver_color,
        );
        if let Some(a) = displays
            .iter()
            .find(|d| d.id == p.anchor_id && p.side != Side::Auto)
        {
            let a = mapped(a.rect);
            let (from, to) = match p.side {
                Side::Right => (
                    egui::pos2(r.left(), r.top().max(a.top())),
                    egui::pos2(r.left(), r.bottom().min(a.bottom())),
                ),
                Side::Left => (
                    egui::pos2(r.right(), r.top().max(a.top())),
                    egui::pos2(r.right(), r.bottom().min(a.bottom())),
                ),
                Side::Above => (
                    egui::pos2(r.left().max(a.left()), r.bottom()),
                    egui::pos2(r.right().min(a.right()), r.bottom()),
                ),
                Side::Below => (
                    egui::pos2(r.left().max(a.left()), r.top()),
                    egui::pos2(r.right().min(a.right()), r.top()),
                ),
                Side::Auto => unreachable!(),
            };
            ui.painter()
                .line_segment([from, to], egui::Stroke::new(4_f32, ACCENT));
        }
    }
    clicked
}
