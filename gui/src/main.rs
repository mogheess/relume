#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod home;
mod inspector;
mod preview;
mod recovery;
mod results;
mod scanview;
mod theme;
#[cfg(windows)]
mod wic;

fn run(renderer: eframe::Renderer) -> eframe::Result<()> {
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Relume")
        .with_min_inner_size([960.0, 640.0])
        .with_icon(theme::app_icon());
    // dev-hooks: RELUME_SIZE=1280x800 opens at a fixed size. Otherwise start maximized.
    let size = if cfg!(feature = "dev-hooks") { std::env::var("RELUME_SIZE").ok() } else { None };
    match size.and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?])
    }) {
        Some(size) => viewport = viewport.with_inner_size(size),
        None => viewport = viewport.with_inner_size([1360.0, 860.0]).with_maximized(true),
    }
    let opts = eframe::NativeOptions { viewport, renderer, ..Default::default() };
    eframe::run_native("Relume", opts, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

fn main() -> eframe::Result<()> {
    // wgpu (DirectX 12 / Vulkan, with a software fallback) works on VMs and remote desktops
    // where OpenGL 2 isn't available; OpenGL is the fallback if wgpu can't start.
    // RELUME_RENDERER=gl forces OpenGL (troubleshooting).
    if std::env::var("RELUME_RENDERER").is_ok_and(|v| v == "gl") {
        return run(eframe::Renderer::Glow);
    }
    match run(eframe::Renderer::Wgpu) {
        Err(e) => {
            eprintln!("wgpu renderer failed ({e}); falling back to OpenGL");
            run(eframe::Renderer::Glow)
        }
        ok => ok,
    }
}
