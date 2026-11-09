#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod ui;

fn options(renderer: eframe::Renderer) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("LinkDesk · 局域网扩展屏")
            .with_inner_size([1160., 680.])
            .with_min_inner_size([900., 580.]),
        renderer,
        ..Default::default()
    }
}

fn run(renderer: eframe::Renderer) -> eframe::Result {
    eframe::run_native(
        "LinkDesk",
        options(renderer),
        Box::new(|cc| Ok(Box::new(ui::LinkDesk::new(cc)))),
    )
}

fn main() {
    // Native Direct3D / Metal / Vulkan rendering first. Keep OpenGL available
    // for older adapters, without requiring a command line for normal startup.
    let force_opengl = std::env::args().any(|arg| arg == "--opengl");
    let result = if force_opengl {
        run(eframe::Renderer::Glow)
    } else {
        run(eframe::Renderer::Wgpu).or_else(|_| run(eframe::Renderer::Glow))
    };
    if let Err(error) = result {
        eprintln!("LinkDesk startup failed: {error}");
        #[cfg(windows)]
        show_startup_error(&error.to_string());
    }
}

#[cfg(windows)]
fn show_startup_error(error: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(window: isize, text: *const u16, title: *const u16, kind: u32) -> i32;
    }
    let message = format!("无法创建原生图形窗口。请检查显卡驱动是否正常。\n\n错误详情：{error}");
    let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "LinkDesk 启动失败".encode_utf16().chain(Some(0)).collect();
    // Both buffers remain NUL-terminated and alive throughout this synchronous call.
    unsafe {
        MessageBoxW(0, message.as_ptr(), title.as_ptr(), 0x10);
    }
}
