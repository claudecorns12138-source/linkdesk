#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use eframe::egui;
use linkdesk::{
    discovery::{self, Device},
    network::{self, Event, Host, LatestFrame, Receiver},
    protocol,
    source::Mode,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver as Events, SyncSender},
        Arc,
    },
    time::Duration,
};

struct Request {
    id: u64,
    name: String,
    decision: SyncSender<bool>,
}
struct LinkDesk {
    host: Option<Host>,
    receiver: Option<Receiver>,
    mode: Mode,
    tx: mpsc::Sender<Event>,
    events: Events<Event>,
    devices: Events<Device>,
    discovered: BTreeMap<String, Device>,
    discovery_stop: Arc<AtomicBool>,
    pending: Vec<Request>,
    peers: BTreeMap<u64, String>,
    address: String,
    pin: String,
    status: String,
    certificate: Option<(String, SyncSender<bool>)>,
    latest: LatestFrame,
    texture: Option<egui::TextureHandle>,
    frames: u64,
    fullscreen: bool,
    show_private: bool,
    demo: bool,
}

impl LinkDesk {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        let (tx, events) = mpsc::channel();
        let (dtx, devices) = mpsc::channel();
        let discovery_stop = Arc::new(AtomicBool::new(false));
        let status = if let Err(e) = discovery::listen(dtx, discovery_stop.clone()) {
            format!("Automatic discovery unavailable ({e}); enter the host IP manually.")
        } else {
            "Ready · local network only".into()
        };
        let mut app = Self {
            host: None,
            receiver: None,
            mode: if cfg!(target_os = "macos") {
                Mode::Extend
            } else {
                Mode::Mirror
            },
            tx,
            events,
            devices,
            discovered: BTreeMap::new(),
            discovery_stop,
            pending: Vec::new(),
            peers: BTreeMap::new(),
            address: String::new(),
            pin: String::new(),
            status,
            certificate: None,
            latest: Arc::default(),
            texture: None,
            frames: 0,
            fullscreen: false,
            show_private: false,
            demo: std::env::args().any(|a| a == "--demo"),
        };
        if app.demo {
            match network::start_host("127.0.0.1:0", Mode::Test, app.tx.clone()) {
                Ok(host) => {
                    app.receiver = Some(network::start_receiver(
                        host.address.to_string(),
                        host.pin(),
                        app.tx.clone(),
                        app.latest.clone(),
                    ));
                    app.host = Some(host);
                }
                Err(e) => app.status = e.to_string(),
            }
        }
        app
    }
    fn stop(&mut self) {
        self.host = None;
        self.receiver = None;
        self.pending.clear();
        self.peers.clear();
        self.certificate = None;
        self.texture = None;
        *self.latest.lock().unwrap() = None;
        self.frames = 0;
        self.status = "Disconnected".into();
    }
    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::PairRequest { id, name, decision } => {
                    if self.demo {
                        let _ = decision.try_send(true);
                    } else {
                        self.pending.push(Request { id, name, decision });
                    }
                }
                Event::PeerConnected { id, name } => {
                    self.pending.retain(|r| r.id != id);
                    self.peers.insert(id, name);
                }
                Event::PeerDisconnected { id } => {
                    self.pending.retain(|r| r.id != id);
                    self.peers.remove(&id);
                }
                Event::Certificate {
                    fingerprint,
                    decision,
                } => {
                    if self.demo {
                        let matches = self
                            .host
                            .as_ref()
                            .is_some_and(|h| h.fingerprint == fingerprint);
                        let _ = decision.try_send(matches);
                    } else {
                        self.certificate = Some((fingerprint, decision));
                    }
                }
                Event::Status(message) => self.status = message,
                Event::SourceError(message) => {
                    self.host = None;
                    self.pending.clear();
                    self.peers.clear();
                    self.status = message;
                }
            }
        }
        while let Ok(device) = self.devices.try_recv() {
            self.discovered.insert(device.address.clone(), device);
        }
        self.discovered
            .retain(|_, d| d.seen.elapsed() < Duration::from_secs(8));
        if let Some(frame) = self.latest.lock().unwrap().take() {
            let image = egui::ColorImage::from_rgb([frame.width, frame.height], &frame.rgb);
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture =
                    Some(ctx.load_texture("native-video", image, egui::TextureOptions::LINEAR));
            }
            self.frames = frame.sequence + 1;
        }
    }
}

impl Drop for LinkDesk {
    fn drop(&mut self) {
        self.discovery_stop.store(true, Ordering::Relaxed);
        self.stop();
    }
}

impl eframe::App for LinkDesk {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        ctx.request_repaint_after(Duration::from_millis(16));
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.fullscreen {
            self.fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if self.fullscreen {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ctx, |ui| {
                    if let Some(texture) = &self.texture {
                        ui.add(
                            egui::Image::new(texture)
                                .fit_to_exact_size(fit(texture.size_vec2(), ui.available_size())),
                        );
                    }
                });
            return;
        }
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(12.);
            ui.horizontal(|ui| {
                ui.heading("LinkDesk");
                ui.label(
                    egui::RichText::new("NATIVE / LAN")
                        .color(egui::Color32::from_rgb(71, 224, 190)),
                );
            });
            ui.label("A real desktop on another device. No browser. No cloud relay.");
            ui.add_space(10.);
        });
        egui::SidePanel::left("controls")
            .min_width(310.)
            .max_width(380.)
            .show(ctx, |ui| {
                ui.add_space(12.);
                ui.heading("Share this computer");
                ui.add_space(8.);
                ui.add_enabled_ui(self.host.is_none() && self.receiver.is_none(), |ui| {
                    if cfg!(target_os = "macos") {
                        ui.radio_value(
                            &mut self.mode,
                            Mode::Extend,
                            "Extend desktop · Mac experimental",
                        );
                    }
                    if cfg!(any(windows, target_os = "macos")) {
                        ui.radio_value(&mut self.mode, Mode::Mirror, "Mirror primary display");
                    }
                    ui.radio_value(
                        &mut self.mode,
                        Mode::Test,
                        "Connection test · synthetic video",
                    );
                    if ui.button("Start sharing").clicked() {
                        match network::start_host(
                            &format!("0.0.0.0:{}", protocol::PORT),
                            self.mode,
                            self.tx.clone(),
                        ) {
                            Ok(host) => {
                                self.host = Some(host);
                                self.status = "Sharing · approve the receiver below".into();
                            }
                            Err(e) => self.status = format!("{e:#}"),
                        }
                    }
                });
                if let Some(host) = &self.host {
                    ui.add_space(8.);
                    ui.checkbox(
                        &mut self.show_private,
                        "Show pairing details (keep private)",
                    );
                    if self.show_private {
                        ui.label("Pairing code");
                        ui.label(
                            egui::RichText::new(host.pin())
                                .size(34.)
                                .monospace()
                                .color(egui::Color32::from_rgb(71, 224, 190)),
                        );
                        ui.label(format!("Expires in {} seconds", host.remaining()));
                        if ui.small_button("New code").clicked() {
                            host.rotate();
                        }
                        ui.label("Certificate fingerprint · compare on receiver");
                        ui.add(
                            egui::Label::new(egui::RichText::new(&host.fingerprint).monospace())
                                .wrap(),
                        );
                    }
                    ui.label(format!(
                        "Native port {} · {} frames",
                        host.address.port(),
                        host.frames.load(Ordering::Relaxed)
                    ));
                    for request in &self.pending {
                        ui.group(|ui| {
                            ui.label(format!("{} wants to connect", request.name));
                            ui.horizontal(|ui| {
                                if ui.button("Allow").clicked() {
                                    let _ = request.decision.try_send(true);
                                }
                                if ui.button("Decline").clicked() {
                                    let _ = request.decision.try_send(false);
                                }
                            });
                        });
                    }
                    for name in self.peers.values() {
                        ui.label(format!("● {name}"));
                    }
                }
                ui.add_space(16.);
                ui.separator();
                ui.add_space(12.);
                ui.heading("Use this screen as a receiver");
                ui.add_enabled_ui(self.receiver.is_none() && self.host.is_none(), |ui| {
                    ui.label("Discovered hosts");
                    if self.discovered.is_empty() {
                        ui.weak("No beacon yet. Manual IP also works.");
                    }
                    for device in self.discovered.values() {
                        if ui
                            .button(format!("{} · {}", device.name, device.address))
                            .clicked()
                        {
                            self.address = device.address.clone();
                        }
                    }
                    ui.label("Host IP and port");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.address).hint_text("<HOST_IP>:47831"),
                    );
                    ui.label("Pairing code from host");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pin)
                            .password(true)
                            .char_limit(6),
                    );
                    if ui.button("Connect").clicked() {
                        if self.pin.len() != 6
                            || !self.pin.bytes().all(|c| c.is_ascii_digit())
                            || self.address.trim().is_empty()
                        {
                            self.status = "Enter a host address and its six-digit code".into();
                        } else {
                            let address = if self.address.contains(':') {
                                self.address.trim().to_owned()
                            } else {
                                format!("{}:{}", self.address.trim(), protocol::PORT)
                            };
                            self.receiver = Some(network::start_receiver(
                                address,
                                self.pin.clone(),
                                self.tx.clone(),
                                self.latest.clone(),
                            ));
                            self.status = "Checking the host certificate…".into();
                        }
                    }
                });
                let mut certificate_choice = None;
                if let Some((fingerprint, _decision)) = &self.certificate {
                    ui.add_space(8.);
                    ui.group(|ui| {
                        ui.label("Compare this fingerprint with the host");
                        ui.add(
                            egui::Label::new(egui::RichText::new(fingerprint).monospace()).wrap(),
                        );
                        if ui.button("Fingerprint matches · pair").clicked() {
                            certificate_choice = Some(true);
                        }
                        if ui.button("Cancel pairing").clicked() {
                            certificate_choice = Some(false);
                        }
                    });
                }
                if let Some(choice) = certificate_choice {
                    if let Some((_, decision)) = self.certificate.take() {
                        let _ = decision.try_send(choice);
                    }
                }
                ui.add_space(16.);
                if (self.host.is_some() || self.receiver.is_some())
                    && ui.button("Disconnect / stop sharing").clicked()
                {
                    self.stop();
                }
                ui.add_space(8.);
                ui.separator();
                ui.label(&self.status);
            });
        egui::CentralPanel::default().show(ctx,|ui|{
            if let Some(texture)=&self.texture {
                ui.horizontal(|ui| {
                    ui.label(format!("{} × {} · frame {}",texture.size()[0],texture.size()[1],self.frames));
                    if self.demo { ui.label("DEMO · generated video, no desktop captured"); }
                    if ui.button("Full screen").clicked() {
                        self.fullscreen=true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    }
                });
                ui.add(egui::Image::new(texture).fit_to_exact_size(fit(texture.size_vec2(),ui.available_size())));
            }else{ui.vertical_centered(|ui|{ui.add_space(100.);ui.heading("Your next display");ui.add_space(12.);ui.label("Connect to a host to receive its native H.264 stream.");ui.label("On Mac, Extend creates a separate system display.");ui.add_space(16.);ui.weak("iPad / iPhone native receiver and Windows virtual display driver: planned.");});}
        });
    }
}

fn fit(source: egui::Vec2, available: egui::Vec2) -> egui::Vec2 {
    source
        * (available.x / source.x)
            .min(available.y / source.y)
            .max(0.01)
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LinkDesk · Native LAN Display")
            .with_inner_size([1180., 760.])
            .with_min_inner_size([850., 600.]),
        ..Default::default()
    };
    eframe::run_native(
        "LinkDesk",
        options,
        Box::new(|cc| Ok(Box::new(LinkDesk::new(cc)))),
    )
}
