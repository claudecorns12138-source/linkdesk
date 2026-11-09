use eframe::egui::{self, Color32, FontFamily, FontId, RichText, Vec2};
use linkdesk::{
    discovery::{self, Device},
    network::{self, Event, Host, LatestFrame, Receiver, ReceiverPhase},
    onboarding, protocol,
    source::Mode,
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver as Events, SyncSender},
        Arc,
    },
    time::{Duration, Instant},
};

const BG: Color32 = Color32::from_rgb(14, 23, 29);
const RAIL: Color32 = Color32::from_rgb(11, 18, 23);
const CARD: Color32 = Color32::from_rgb(22, 35, 43);
const BORDER: Color32 = Color32::from_rgb(43, 62, 73);
const INK: Color32 = Color32::from_rgb(228, 239, 244);
const MUTED: Color32 = Color32::from_rgb(143, 166, 179);
const ACCENT: Color32 = Color32::from_rgb(95, 231, 202);
const ERROR: Color32 = Color32::from_rgb(255, 161, 149);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Receive,
    Share,
}
struct Request {
    id: u64,
    name: String,
    decision: SyncSender<bool>,
    created: Instant,
}
struct Certificate {
    fingerprint: String,
    decision: SyncSender<bool>,
    changed: bool,
}
struct Notice {
    title: String,
    hint: String,
    detail: String,
}

pub struct LinkDesk {
    desktop: crate::layout_ui::LayoutControls,
    layout_preview: bool,
    role: Role,
    mode: Mode,
    host: Option<Host>,
    host_events: Option<Events<Event>>,
    addresses: Vec<String>,
    receiver: Option<Receiver>,
    receiver_events: Option<Events<Event>>,
    phase: Option<ReceiverPhase>,
    devices: Events<Device>,
    discovered: BTreeMap<String, Device>,
    discovery_stop: Arc<AtomicBool>,
    discovery_error: bool,
    requests: Vec<Request>,
    peers: BTreeMap<u64, String>,
    certificate: Option<Certificate>,
    trusted: BTreeMap<String, String>,
    address: String,
    active_address: String,
    pin: String,
    reveal_pin: bool,
    show_private: bool,
    latest: LatestFrame,
    texture: Option<egui::TextureHandle>,
    frame_size: [usize; 2],
    frames: u64,
    sampling: Instant,
    sample_counts: (u64, u64),
    fps: f64,
    mbps: f64,
    notice: Option<Notice>,
    toast: Option<(String, Instant)>,
    fullscreen: bool,
    fullscreen_since: Instant,
    last_pointer: Option<egui::Pos2>,
    last_pointer_move: Instant,
    demo: bool,
}

impl LinkDesk {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure(&cc.egui_ctx);
        let (tx, devices) = mpsc::sync_channel(32);
        let discovery_stop = Arc::new(AtomicBool::new(false));
        let discovery_error = discovery::listen(tx, discovery_stop.clone()).is_err();
        let now = Instant::now();
        let mut app = Self {
            desktop: crate::layout_ui::LayoutControls::default(),
            layout_preview: std::env::args().any(|a| a == "--layout-preview"),
            role: if cfg!(target_os = "macos") {
                Role::Share
            } else {
                Role::Receive
            },
            mode: if cfg!(target_os = "macos") {
                Mode::Extend
            } else {
                Mode::Mirror
            },
            host: None,
            host_events: None,
            addresses: Vec::new(),
            receiver: None,
            receiver_events: None,
            phase: None,
            devices,
            discovered: BTreeMap::new(),
            discovery_stop,
            discovery_error,
            requests: Vec::new(),
            peers: BTreeMap::new(),
            certificate: None,
            trusted: BTreeMap::new(),
            address: String::new(),
            active_address: String::new(),
            pin: String::new(),
            reveal_pin: false,
            show_private: false,
            latest: Arc::default(),
            texture: None,
            frame_size: [0, 0],
            frames: 0,
            sampling: now,
            sample_counts: (0, 0),
            fps: 0.,
            mbps: 0.,
            notice: None,
            toast: None,
            fullscreen: false,
            fullscreen_since: now,
            last_pointer: None,
            last_pointer_move: now,
            demo: std::env::args().any(|a| a == "--demo" || a == "--layout-preview"),
        };
        if app.layout_preview {
            app.desktop = crate::layout_ui::LayoutControls::preview();
            app.role = Role::Share;
            app.mode = Mode::Test;
        } else if app.demo {
            app.start_demo();
        }
        app
    }

    fn start_host(&mut self, bind: &str) {
        self.notice = None;
        // Automatic approvals belong exclusively to a synthetic loopback host.
        // Enforce this here too, regardless of the currently selected UI mode.
        let (bind, mode) = if self.demo {
            ("127.0.0.1:0", Mode::Test)
        } else {
            (bind, self.mode)
        };
        let (tx, events) = mpsc::channel();
        match network::start_host_with_layout(bind, mode, self.desktop.options(), tx) {
            Ok(host) => {
                self.addresses = if host.address.ip().is_loopback() {
                    vec![host.address.to_string()]
                } else {
                    discovery::local_addresses(host.address.port())
                };
                self.host = Some(host);
                self.host_events = Some(events);
            }
            Err(error) => {
                self.notice = Some(Notice {
                    title: "无法开始共享".into(),
                    hint: "检查是否已有 LinkDesk 主机运行，或端口被其他程序占用。".into(),
                    detail: format!("{error:#}"),
                })
            }
        }
    }

    fn start_demo(&mut self) {
        self.stop_receiver();
        self.stop_host();
        self.mode = Mode::Test;
        self.role = Role::Receive;
        self.start_host("127.0.0.1:0");
        if let Some(host) = &self.host {
            self.address = host.address.to_string();
            self.pin = host.pin();
            self.connect();
        }
    }

    fn end_demo(&mut self) {
        self.stop_receiver();
        self.stop_host();
        self.demo = false;
        self.layout_preview = false;
        self.desktop = crate::layout_ui::LayoutControls::default();
        self.mode = if cfg!(target_os = "macos") {
            Mode::Extend
        } else {
            Mode::Mirror
        };
        self.address.clear();
        self.active_address.clear();
        self.pin.clear();
        self.notice = None;
    }

    fn connect(&mut self) {
        let (address, pin) = match (
            onboarding::address(&self.address),
            onboarding::pin(&self.pin),
        ) {
            (Ok(address), Ok(pin)) => (address, pin),
            (Err(error), _) | (_, Err(error)) => {
                self.notice = Some(Notice {
                    title: "检查连接信息".into(),
                    hint: error.into(),
                    detail: String::new(),
                });
                return;
            }
        };
        self.stop_receiver();
        self.notice = None;
        self.address = address.clone();
        self.active_address = address.clone();
        self.pin = pin.clone();
        // Each attempt owns its event channel and frame slot. Old completion and
        // in-flight decoded frames cannot modify a newer connection attempt.
        self.latest = Arc::default();
        let (tx, events) = mpsc::channel();
        self.receiver = Some(network::start_receiver_with_trust(
            address.clone(),
            pin,
            self.trusted.get(&address).cloned(),
            tx,
            self.latest.clone(),
        ));
        self.receiver_events = Some(events);
        self.phase = Some(ReceiverPhase::Connecting);
        self.sample_counts = (0, 0);
        self.sampling = Instant::now();
        self.fps = 0.;
        self.mbps = 0.;
    }

    fn stop_receiver(&mut self) {
        self.receiver_events = None;
        self.receiver = None;
        self.phase = None;
        self.certificate = None;
        self.latest = Arc::default();
        self.texture = None;
        self.frames = 0;
        self.fps = 0.;
        self.mbps = 0.;
        self.frame_size = [0, 0];
    }
    fn stop_host(&mut self) {
        self.desktop.clear_live();
        self.host_events = None;
        self.host = None;
        self.requests.clear();
        self.peers.clear();
        self.show_private = false;
    }
    fn leave_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }
    fn copy(&mut self, ctx: &egui::Context, value: String, message: &str) {
        ctx.copy_text(value);
        self.toast = Some((message.into(), Instant::now()));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Some(event) = self.host_events.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match event {
                Event::DesktopReady(layout) => self.desktop.ready(layout),
                Event::PairRequest { id, name, decision } => {
                    if self.demo {
                        let _ = decision.try_send(true);
                    } else {
                        self.requests.push(Request {
                            id,
                            name,
                            decision,
                            created: Instant::now(),
                        });
                    }
                }
                Event::PeerConnected { id, name } => {
                    self.requests.retain(|r| r.id != id);
                    self.peers.insert(id, name);
                }
                Event::PeerDisconnected { id } => {
                    self.requests.retain(|r| r.id != id);
                    self.peers.remove(&id);
                }
                Event::SourceError(detail) => {
                    self.stop_host();
                    self.notice = Some(Notice {
                        title: "共享未能启动".into(),
                        hint: if cfg!(target_os = "macos") {
                            "请检查屏幕录制权限和屏幕排列，尝试刷新后更换空闲边缘。若现有屏幕状态变化，本次扩展会停止。"
                        } else {
                            "请检查显卡驱动、显示器和当前桌面是否可用。"
                        }
                        .into(),
                        detail,
                    });
                }
                _ => {}
            }
        }
        while let Some(event) = self
            .receiver_events
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            match event {
                Event::ReceiverState(phase) => self.phase = Some(phase),
                Event::Certificate {
                    fingerprint,
                    decision,
                    changed,
                } => {
                    if self.demo {
                        let matches = self
                            .host
                            .as_ref()
                            .is_some_and(|h| h.fingerprint == fingerprint);
                        let _ = decision.try_send(matches);
                    } else {
                        self.certificate = Some(Certificate {
                            fingerprint,
                            decision,
                            changed,
                        });
                    }
                }
                Event::ReceiverFinished { error } => {
                    let streamed = self.frames > 0;
                    self.stop_receiver();
                    if self.fullscreen {
                        self.leave_fullscreen(ctx);
                    }
                    self.notice=Some(Notice{title:if streamed{"连接已断开"}else{"连接未完成"}.into(),hint:if streamed{"主机可能已结束共享或网络发生变化。确认主机仍在共享后，点击重新连接。"}else{"请检查主机地址、配对码是否过期，并在主机上允许本次连接。热点隔离或防火墙也可能阻止连接。"}.into(),detail:error.unwrap_or_default()});
                }
                _ => {}
            }
        }
        while let Ok(device) = self.devices.try_recv() {
            if self.discovered.len() < 24 || self.discovered.contains_key(&device.address) {
                self.discovered.insert(device.address.clone(), device);
            }
        }
        self.discovered
            .retain(|_, device| device.seen.elapsed() < Duration::from_secs(8));
        if let Some(frame) = self.latest.lock().unwrap().take() {
            self.frame_size = [frame.width, frame.height];
            let image = egui::ColorImage::from_rgb(self.frame_size, &frame.rgb);
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture =
                    Some(ctx.load_texture("native-display", image, egui::TextureOptions::LINEAR));
            }
            self.frames += 1;
        }
        let elapsed = self.sampling.elapsed().as_secs_f64();
        if elapsed >= 1. {
            if let Some(receiver) = &self.receiver {
                let counts = receiver.stats.counts();
                self.fps = counts.0.saturating_sub(self.sample_counts.0) as f64 / elapsed;
                self.mbps = counts.1.saturating_sub(self.sample_counts.1) as f64 * 8.
                    / elapsed
                    / 1_000_000.;
                self.sample_counts = counts;
            }
            self.sampling = Instant::now();
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(3))
        {
            self.toast = None;
        }
    }

    fn rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigation")
            .exact_width(182.)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(RAIL).inner_margin(20))
            .show(ctx, |ui| {
                ui.add_space(8.);
                ui.label(RichText::new("LinkDesk").size(25.).strong().color(INK));
                ui.label(RichText::new("让屏幕彼此相连").size(12.).color(MUTED));
                ui.add_space(34.);
                nav(
                    ui,
                    "接收屏幕",
                    "将本机用作第二块屏",
                    self.role == Role::Receive,
                    || self.role = Role::Receive,
                );
                ui.add_space(10.);
                nav(
                    ui,
                    "共享本机",
                    "向其他设备发送画面",
                    self.role == Role::Share,
                    || self.role = Role::Share,
                );
                ui.add_space(30.);
                ui.separator();
                ui.add_space(16.);
                pill(ui, "原生应用 · 本地直连", ACCENT);
                ui.add_space(14.);
                ui.label(RichText::new("首次使用").strong());
                ui.label(
                    RichText::new("主机开始共享，再在接收端选择设备并配对。")
                        .color(MUTED)
                        .size(13.),
                );
                ui.add_space(16.);
                ui.label(
                    RichText::new("操作画面仍使用主机的键盘和鼠标。")
                        .color(MUTED)
                        .size(12.),
                );
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(RichText::new("0.1.0 Alpha 3").color(MUTED).size(11.));
                    ui.label(
                        RichText::new("Mac 扩展屏仍为实验功能")
                            .color(MUTED)
                            .size(11.),
                    );
                });
            });
    }

    fn receive_controls(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("pairing")
            .exact_width(330.)
            .resizable(false)
            .frame(egui::Frame::NONE.fill(BG).inner_margin(20))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(8.);
                    ui.heading("连接另一台电脑");
                    ui.label(
                        RichText::new("主机与接收端需在可互通的局域网中。")
                            .color(MUTED)
                            .size(12.),
                    );
                    ui.add_space(18.);
                    if let Some(phase) = self.phase {
                        phase_card(ui, phase);
                        ui.add_space(16.);
                    }
                    if let Some(certificate) = &self.certificate {
                        let mut choice = None;
                        card()
                            .stroke(egui::Stroke::new(
                                1_f32,
                                if certificate.changed { ERROR } else { ACCENT },
                            ))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(if certificate.changed {
                                        "主机身份已变化"
                                    } else {
                                        "确认这台主机"
                                    })
                                    .strong()
                                    .size(18.),
                                );
                                ui.label(
                                    RichText::new(
                                        "对照主机上显示的完整指纹。相同后再允许发送配对信息。",
                                    )
                                    .color(MUTED)
                                    .size(12.),
                                );
                                ui.add_space(8.);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&certificate.fingerprint)
                                            .monospace()
                                            .size(12.),
                                    )
                                    .wrap(),
                                );
                                ui.add_space(8.);
                                if primary(ui, "指纹一致，继续配对", true).clicked() {
                                    choice = Some(true);
                                }
                                if ui.button("取消本次连接").clicked() {
                                    choice = Some(false);
                                }
                            });
                        if let Some(allow) = choice {
                            if let Some(certificate) = self.certificate.take() {
                                if allow {
                                    self.trusted.insert(
                                        self.active_address.clone(),
                                        certificate.fingerprint,
                                    );
                                }
                                let _ = certificate.decision.try_send(allow);
                                if !allow {
                                    self.stop_receiver();
                                }
                            }
                        }
                        ui.add_space(18.);
                    }
                    let busy = self.receiver.is_some();
                    if self.demo && !busy {
                        ui.label(RichText::new("本机连接诊断已停止").strong());
                        ui.label("诊断仅使用合成画面，不采集桌面、不向局域网开放。");
                        ui.add_space(12.);
                        if primary(ui, "重新运行诊断", true).clicked() {
                            self.start_demo();
                        }
                        if ui.button("退出诊断，连接其他电脑").clicked() {
                            self.end_demo();
                        }
                    } else if !busy {
                        ui.add_enabled_ui(self.host.is_none(), |ui| {
                            ui.label(RichText::new("附近的主机").strong());
                            if self.discovered.is_empty() {
                                card().show(ui, |ui| {
                                    ui.label(
                                        RichText::new("正在寻找共享中的电脑…")
                                            .color(MUTED)
                                            .size(13.),
                                    );
                                    ui.label(
                                        RichText::new("也可以直接输入地址").color(MUTED).size(12.),
                                    );
                                });
                            }
                            let mut selected = None;
                            for device in self.discovered.values() {
                                if ui
                                    .add_sized(
                                        [ui.available_width(), 54.],
                                        egui::Button::new(format!(
                                            "{}\n{}",
                                            device.name, device.address
                                        ))
                                        .selected(self.address == device.address),
                                    )
                                    .clicked()
                                {
                                    selected = Some(device.address.clone());
                                }
                            }
                            if let Some(address) = selected {
                                self.address = address;
                                self.notice = None;
                            }
                            ui.add_space(12.);
                            ui.label("主机地址");
                            ui.add_sized(
                                [ui.available_width(), 36.],
                                egui::TextEdit::singleline(&mut self.address)
                                    .hint_text("IP 或主机名，端口可省略"),
                            );
                            ui.add_space(6.);
                            ui.horizontal(|ui| {
                                ui.label("六位配对码");
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.checkbox(&mut self.reveal_pin, "显示");
                                    },
                                );
                            });
                            let pin = ui.add_sized(
                                [ui.available_width(), 38.],
                                egui::TextEdit::singleline(&mut self.pin)
                                    .password(!self.reveal_pin)
                                    .char_limit(16)
                                    .hint_text("在主机上查看"),
                            );
                            ui.add_space(12.);
                            if primary(
                                ui,
                                if self.notice.is_some() {
                                    "重新连接"
                                } else {
                                    "连接主机"
                                },
                                true,
                            )
                            .clicked()
                                || (pin.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                self.connect();
                            }
                        });
                    } else {
                        ui.label(
                            RichText::new(if self.demo {
                                "本机连接诊断"
                            } else {
                                "当前主机"
                            })
                            .strong(),
                        );
                        if !self.demo {
                            ui.monospace(&self.active_address);
                        }
                        ui.label(
                            RichText::new("F11 进入全屏，Esc 返回。\n操作仍使用主机的键盘和鼠标。")
                                .color(MUTED)
                                .size(13.),
                        );
                    }
                    if busy {
                        ui.add_space(12.);
                        if ui
                            .add_sized(
                                [ui.available_width(), 36.],
                                egui::Button::new(if self.texture.is_some() {
                                    "断开连接"
                                } else {
                                    "取消连接"
                                }),
                            )
                            .clicked()
                        {
                            self.stop_receiver();
                            if self.demo {
                                self.stop_host();
                            }
                            self.notice = None;
                        }
                    }
                    ui.add_space(18.);
                    self.notice_ui(ui);
                    if self.discovery_error {
                        ui.label(
                            RichText::new("自动发现暂不可用，手动填写主机地址仍可连接。")
                                .color(MUTED)
                                .size(12.),
                        );
                    }
                    ui.add_space(18.);
                    ui.collapsing("找不到主机或连接不上？", |ui| {
                        ui.label("1. 确认另一台电脑已开始共享。");
                        ui.label("2. 尝试填写主机的实际 IP，勿填手机或路由器地址。");
                        ui.label("3. 同一个热点也可能隔离设备。可用普通路由器或网线作对照。");
                        ui.label("4. 检查应用的本地网络权限和防火墙放行规则。");
                    });
                    ui.add_space(12.);
                    ui.label(
                        RichText::new("设备信任仅保留在本次应用运行期间。")
                            .size(11.)
                            .color(MUTED),
                    );
                });
            });
    }

    fn notice_ui(&self, ui: &mut egui::Ui) {
        if let Some(notice) = &self.notice {
            card()
                .stroke(egui::Stroke::new(1_f32, Color32::from_rgb(111, 66, 62)))
                .show(ui, |ui| {
                    ui.label(RichText::new(&notice.title).strong().color(ERROR));
                    ui.label(RichText::new(&notice.hint).size(13.).color(MUTED));
                    if !notice.detail.is_empty() {
                        ui.collapsing("查看错误详情", |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&notice.detail).size(11.).monospace(),
                                )
                                .wrap(),
                            );
                        });
                    }
                });
        }
    }

    fn preview(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG).inner_margin(26))
            .show(ctx, |ui| {
                ui.add_space(4.);
                ui.heading("把这块屏幕，接入你的工作空间");
                ui.label(
                    RichText::new(if self.demo {
                        "连接诊断正在使用合成画面，不会采集桌面。"
                    } else {
                        "连接后，主机的画面会显示在这里。"
                    })
                    .color(MUTED)
                    .size(13.),
                );
                ui.add_space(20.);
                if self.texture.is_some() {
                    ui.horizontal(|ui| {
                        pill(ui, "已加密 · 本地直连", ACCENT);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("进入全屏  F11").clicked() {
                                self.enter_fullscreen(ctx);
                            }
                        });
                    });
                    ui.add_space(14.);
                    ui.horizontal(|ui| {
                        metric(
                            ui,
                            "分辨率",
                            format!("{} × {}", self.frame_size[0], self.frame_size[1]),
                        );
                        metric(ui, "解码帧率", format!("{:.0} FPS", self.fps));
                        metric(ui, "视频码率", format!("{:.1} Mbps", self.mbps));
                    });
                    ui.add_space(14.);
                    let size = Vec2::new(
                        ui.available_width(),
                        (ui.available_height() - 28.).max(120.),
                    );
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    self.paint_video(ui, rect);
                    ui.label(
                        RichText::new("静态桌面可能降低更新频率；帧率不代表端到端延迟。")
                            .color(MUTED)
                            .size(11.),
                    );
                } else {
                    let height = (ui.available_height() - 90.).max(220.);
                    card().show(ui, |ui| {
                        ui.set_min_height(height);
                        ui.set_width(ui.available_width());
                        ui.vertical_centered(|ui| {
                            ui.add_space((height / 2. - 94.).max(20.));
                            screens(ui);
                            ui.add_space(18.);
                            ui.label(
                                RichText::new(if self.receiver.is_some() {
                                    "正在建立显示连接"
                                } else {
                                    "等待连接你的主机"
                                })
                                .size(21.)
                                .strong(),
                            );
                            ui.add_space(8.);
                            ui.label(
                                RichText::new("先在主机上开始共享，再在右侧选择设备。")
                                    .color(MUTED)
                                    .size(13.),
                            );
                        });
                    });
                    ui.add_space(16.);
                    ui.label(
                        RichText::new("Mac 主机选择“扩展桌面”后，会尝试增加一块独立的系统显示器。")
                            .size(12.)
                            .color(MUTED),
                    );
                }
            });
    }

    fn share(&mut self, ctx: &egui::Context) {
        if self.demo {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(BG).inner_margin(30))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.heading(if self.layout_preview {
                            "屏幕排列预览"
                        } else {
                            "正在进行本机连接诊断"
                        });
                        ui.label(if self.layout_preview {
                            "仅演示布局，不读取真实屏幕、不采集桌面、不向局域网开放。"
                        } else {
                            "诊断仅使用合成画面，不采集桌面、不向局域网开放。"
                        });
                        if self.layout_preview {
                            self.desktop.show_setup(ui, ctx);
                        }
                        ui.add_space(20.);
                        if ui.button("退出诊断，设置共享").clicked() {
                            self.end_demo();
                        }
                    });
                });
            return;
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG).inner_margin(30)).show(ctx,|ui|{
            egui::ScrollArea::vertical().show(ui,|ui|{
                ui.heading("让另一块屏幕，延伸这台电脑");ui.label(RichText::new("选择共享方式，接收端通过配对码连接。").color(MUTED));ui.add_space(24.);
                if self.host.is_none(){
                    card().show(ui,|ui|{
                        ui.label(RichText::new("共享方式").size(18.).strong());ui.add_space(10.);
                        if cfg!(target_os="macos"){mode(ui,&mut self.mode,Mode::Extend,"扩展桌面","新增独立显示器，窗口可拖过去。Mac 实验功能。");}
                        if cfg!(any(windows,target_os="macos")){mode(ui,&mut self.mode,Mode::Mirror,"镜像主屏幕","在另一台设备上显示相同的桌面内容。");}
                        mode(ui,&mut self.mode,Mode::Test,"连接诊断","发送合成画面，先验证网络，不分享真实桌面。");
                        if cfg!(target_os="macos")&&self.mode==Mode::Extend {self.desktop.show_setup(ui,ctx);}
                        ui.add_space(12.);if primary(ui,"开始共享",self.receiver.is_none()).clicked(){self.start_host(&format!("0.0.0.0:{}",protocol::PORT));}
                        if !cfg!(target_os="macos"){ui.label(RichText::new(if cfg!(windows){"Windows 主机的独立扩展屏驱动仍在开发；当前可作为接收端。"}else{"Linux 采集与虚拟显示器尚未实现；当前可接收画面或发送连接诊断。"}).size(12.).color(MUTED));}
                    });
                }else{self.host_controls(ui,ctx);}
                ui.add_space(20.);self.notice_ui(ui);
                ui.add_space(20.);card().show(ui,|ui|{
                    ui.label(RichText::new("Mac + iPad 随航 + 接收屏").strong());ui.label(RichText::new("三块屏显示同一个 Mac 桌面的不同区域。iPad 随航屏上的 Mac 窗口，可沿相接边缘直接拖到接收屏；无需让 iPad 另连 Windows。").size(13.).color(MUTED));
                    ui.label(RichText::new("iPad 原生 App、Windows 本机 App 与文件的跨系统拖放仍未实现。").size(12.).color(MUTED));
                    ui.add_space(6.);ui.label(RichText::new("Mac 实机兼容性与随航并用仍需验收；本版本为开发预览。").size(12.).color(MUTED));
                });
            });
        });
    }

    fn host_controls(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let host = self.host.as_ref().unwrap();
        let port = host.address.port();
        let pin = host.pin();
        let remaining = host.remaining();
        let fingerprint = host.fingerprint.clone();
        let frames = host.frames.load(Ordering::Relaxed);
        card().show(ui, |ui| {
            ui.horizontal(|ui| {
                pill(
                    ui,
                    if frames > 0 {
                        "正在共享"
                    } else {
                        "正在准备采集"
                    },
                    ACCENT,
                );
                ui.label(RichText::new(mode_title(self.mode)).color(MUTED));
            });
            if frames == 0 {
                ui.label(
                    RichText::new("首次使用 Mac 时，请在系统提示中允许屏幕录制与本地网络访问。")
                        .size(13.)
                        .color(MUTED),
                );
            }
            ui.add_space(14.);
            ui.checkbox(&mut self.show_private, "显示配对信息（分享截图前请收起）");
            if self.show_private {
                ui.add_space(12.);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(if remaining == 0 { "已过期" } else { &pin })
                            .font(FontId::monospace(40.))
                            .color(if remaining == 0 { ERROR } else { ACCENT }),
                    );
                    ui.vertical(|ui| {
                        ui.label(if remaining == 0 {
                            "请刷新配对码".into()
                        } else {
                            format!("{:02}:{:02} 后过期", remaining / 60, remaining % 60)
                        });
                        if ui.small_button("换一个配对码").clicked() {
                            if let Some(host) = &self.host {
                                host.rotate();
                            }
                        }
                    });
                    if ui.button("复制配对码").clicked() {
                        self.copy(ctx, pin.clone(), "配对码已复制");
                    }
                });
                ui.add_space(12.);
                ui.label(RichText::new("主机地址").strong());
                if self.addresses.is_empty() {
                    ui.label("未读取到网络地址，可在系统网络设置中查看。");
                }
                let mut copy = None;
                for address in &self.addresses {
                    ui.horizontal(|ui| {
                        ui.monospace(address);
                        if ui.small_button("复制地址").clicked() {
                            copy = Some(address.clone());
                        }
                    });
                }
                if let Some(address) = copy {
                    self.copy(ctx, address, "地址已复制");
                }
                ui.add_space(12.);
                ui.collapsing("身份指纹 · 供接收端核对", |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&fingerprint).monospace().size(12.)).wrap(),
                    );
                    if ui.button("复制完整指纹").clicked() {
                        self.copy(ctx, fingerprint, "指纹已复制");
                    }
                });
            } else {
                ui.add_space(8.);
                ui.label(
                    RichText::new("展开后查看主机地址、六位配对码和身份指纹。")
                        .size(13.)
                        .color(MUTED),
                );
            }
            ui.add_space(14.);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("端口 {port} · 已连接 {} / 4 台", self.peers.len()))
                        .color(MUTED)
                        .size(12.),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("结束共享").clicked() {
                        self.stop_host();
                    }
                });
            });
        });
        if self.mode == Mode::Extend {
            self.desktop.show_live(ui);
        }
        if !self.requests.is_empty() {
            ui.add_space(20.);
            ui.label(RichText::new("等待你允许的连接").strong().size(18.));
            for request in &self.requests {
                card()
                    .stroke(egui::Stroke::new(1_f32, ACCENT))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.strong(&request.name);
                                ui.label(
                                    RichText::new(format!(
                                        "请在 {} 秒内处理",
                                        60u64.saturating_sub(request.created.elapsed().as_secs())
                                    ))
                                    .size(12.)
                                    .color(MUTED),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("允许连接").clicked() {
                                        let _ = request.decision.try_send(true);
                                    }
                                    if ui.button("拒绝").clicked() {
                                        let _ = request.decision.try_send(false);
                                    }
                                },
                            );
                        });
                    });
            }
        }
        if !self.peers.is_empty() {
            ui.add_space(20.);
            ui.label(RichText::new("已连接的接收屏").strong().size(18.));
            let mut disconnect = None;
            for (id, name) in &self.peers {
                card().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("●").color(ACCENT));
                        ui.strong(name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("断开此设备").clicked() {
                                disconnect = Some(*id);
                            }
                        });
                    });
                });
            }
            if let Some(id) = disconnect {
                if let Some(host) = &self.host {
                    host.disconnect(id);
                }
            }
        }
    }

    fn paint_video(&self, ui: &egui::Ui, rect: egui::Rect) {
        ui.painter().rect_filled(rect, 10, Color32::BLACK);
        if let Some(texture) = &self.texture {
            let source = texture.size_vec2();
            let scale = (rect.width() / source.x).min(rect.height() / source.y);
            let image = egui::Rect::from_center_size(rect.center(), source * scale);
            ui.painter().image(
                texture.id(),
                image,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                Color32::WHITE,
            );
        }
    }
    fn enter_fullscreen(&mut self, ctx: &egui::Context) {
        if self.texture.is_some() {
            self.fullscreen = true;
            self.fullscreen_since = Instant::now();
            self.last_pointer_move = Instant::now();
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
    }
    fn fullscreen(&mut self, ctx: &egui::Context) {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        if pointer != self.last_pointer {
            self.last_pointer = pointer;
            self.last_pointer_move = Instant::now();
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ctx.screen_rect();
                self.paint_video(ui, rect);
            });
        if self.last_pointer_move.elapsed() < Duration::from_secs(3)
            || self.fullscreen_since.elapsed() < Duration::from_secs(3)
        {
            egui::Area::new("fullscreen-controls".into())
                .anchor(egui::Align2::RIGHT_TOP, [-20., 20.])
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    card().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Esc / F11 退出全屏").size(12.));
                            if ui.button("退出全屏").clicked() {
                                self.leave_fullscreen(ctx);
                            }
                        });
                    });
                });
        } else {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
    }
}

impl eframe::App for LinkDesk {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.fullscreen {
            self.leave_fullscreen(ctx);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            if self.fullscreen {
                self.leave_fullscreen(ctx);
            } else {
                self.enter_fullscreen(ctx);
            }
        }
        ctx.request_repaint_after(if self.receiver.is_some() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(200)
        });
        if self.fullscreen {
            self.fullscreen(ctx);
            return;
        }
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(RAIL)
                    .inner_margin(egui::Margin::symmetric(20, 9)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("●").color(
                        if self.texture.is_some() || self.host.is_some() {
                            ACCENT
                        } else {
                            MUTED
                        },
                    ));
                    ui.label(
                        RichText::new(if self.demo {
                            "连接诊断 · 合成画面"
                        } else if self.texture.is_some() {
                            "已连接，画面经过加密传输"
                        } else if self.host.is_some() {
                            "共享已启动，等待接收端连接"
                        } else {
                            "已就绪"
                        })
                        .size(12.)
                        .color(MUTED),
                    );
                    if let Some((message, _)) = &self.toast {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(message).color(ACCENT).size(12.));
                        });
                    }
                });
            });
        self.rail(ctx);
        match self.role {
            Role::Receive => {
                self.receive_controls(ctx);
                self.preview(ctx);
            }
            Role::Share => self.share(ctx),
        }
    }
}
impl Drop for LinkDesk {
    fn drop(&mut self) {
        self.discovery_stop.store(true, Ordering::Relaxed);
        self.stop_receiver();
        self.stop_host();
    }
}

fn card() -> egui::Frame {
    egui::Frame::NONE
        .fill(CARD)
        .corner_radius(12)
        .inner_margin(18)
        .stroke(egui::Stroke::new(1_f32, BORDER))
}
fn primary(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(label).strong().color(RAIL))
            .fill(ACCENT)
            .min_size(egui::vec2(ui.available_width(), 42.))
            .corner_radius(8),
    )
}
fn pill(ui: &mut egui::Ui, label: &str, color: Color32) {
    egui::Frame::NONE
        .fill(color.gamma_multiply(0.10))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(9, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(label).color(color).size(12.));
        });
}
fn nav(ui: &mut egui::Ui, title: &str, subtitle: &str, selected: bool, action: impl FnOnce()) {
    let response = ui.add_sized(
        [ui.available_width(), 64.],
        egui::Button::new(
            RichText::new(format!("{title}\n{subtitle}"))
                .size(13.)
                .color(if selected { ACCENT } else { MUTED }),
        )
        .fill(if selected {
            Color32::from_rgb(23, 49, 51)
        } else {
            RAIL
        })
        .corner_radius(10),
    );
    if response.clicked() {
        action();
    }
}
fn metric(ui: &mut egui::Ui, label: &str, value: String) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).color(MUTED).size(11.));
        ui.label(RichText::new(value).size(16.).strong());
    });
    ui.add_space(14.);
}
fn mode_title(mode: Mode) -> &'static str {
    match mode {
        Mode::Extend => "扩展桌面 · Mac 实验功能",
        Mode::Mirror => "镜像主屏幕",
        Mode::Test => "连接诊断 · 合成画面",
    }
}
fn mode(ui: &mut egui::Ui, selected: &mut Mode, value: Mode, title: &str, hint: &str) {
    ui.horizontal(|ui| {
        ui.radio_value(selected, value, RichText::new(title).strong());
    });
    ui.label(RichText::new(hint).color(MUTED).size(12.));
    ui.add_space(8.);
}
fn phase_card(ui: &mut egui::Ui, phase: ReceiverPhase) {
    let (title, hint) = match phase {
        ReceiverPhase::Connecting => ("正在查找主机", "连接主机并读取身份信息。"),
        ReceiverPhase::Verifying => ("核对主机身份", "首次连接需要对照完整指纹。"),
        ReceiverPhase::WaitingApproval => ("等待主机允许", "请在主机端点击“允许连接”。"),
        ReceiverPhase::WaitingVideo => ("配对成功，等待画面", "主机正在准备显示画面。"),
        ReceiverPhase::Streaming => ("显示连接已建立", "可以进入全屏，作为主机的接收屏。"),
    };
    card().show(ui, |ui| {
        ui.horizontal(|ui| {
            if phase != ReceiverPhase::Streaming {
                ui.spinner();
            } else {
                ui.label(RichText::new("●").color(ACCENT));
            }
            ui.strong(title);
        });
        ui.label(RichText::new(hint).color(MUTED).size(12.));
    });
}
fn screens(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(180., 90.), egui::Sense::hover());
    let p = ui.painter();
    let a = egui::Rect::from_min_size(rect.min + egui::vec2(8., 8.), egui::vec2(100., 65.));
    let b = egui::Rect::from_min_size(rect.min + egui::vec2(86., 27.), egui::vec2(82., 54.));
    p.rect_filled(a, 7, BORDER);
    p.rect_filled(a.shrink(2.), 5, Color32::from_rgb(17, 31, 39));
    p.rect_filled(b, 7, ACCENT.gamma_multiply(0.65));
    p.rect_filled(b.shrink(2.), 5, Color32::from_rgb(23, 49, 51));
    p.line_segment(
        [
            a.center_bottom() + egui::vec2(-17., 7.),
            a.center_bottom() + egui::vec2(17., 7.),
        ],
        egui::Stroke::new(3_f32, BORDER),
    );
}
fn configure(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    {
        let fonts = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
            .join("Fonts");
        candidates.push(fonts.join("msyh.ttc"));
        candidates.push(fonts.join("simhei.ttf"));
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/System/Library/Fonts/PingFang.ttc"));
        candidates.push(PathBuf::from("/System/Library/Fonts/STHeiti Medium.ttc"));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        candidates.push(PathBuf::from(
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        ));
        candidates.push(PathBuf::from(
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        ));
    }
    for candidate in candidates {
        if let Ok(bytes) = std::fs::read(candidate) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "system-cjk".into(),
                egui::FontData::from_owned(bytes).into(),
            );
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("system-cjk".into());
            }
            ctx.set_fonts(fonts);
            break;
        }
    }
    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10., 10.);
        style.spacing.button_padding = egui::vec2(12., 8.);
        style.visuals.panel_fill = BG;
        style.visuals.window_fill = CARD;
        style.visuals.extreme_bg_color = RAIL;
        style.visuals.override_text_color = Some(INK);
        style.visuals.selection.bg_fill = Color32::from_rgb(31, 83, 78);
        style.visuals.selection.stroke = egui::Stroke::new(1_f32, ACCENT);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(29, 46, 56);
        style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(29, 46, 56);
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1_f32, BORDER);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(40, 64, 73);
        style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(40, 64, 73);
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1_f32, ACCENT);
        style.text_styles.insert(
            egui::TextStyle::Heading,
            FontId::new(24., FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Body,
            FontId::new(14., FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            FontId::new(14., FontFamily::Proportional),
        );
    });
}
