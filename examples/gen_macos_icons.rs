//! One-shot generator for the macOS tray icon PNGs.
//!
//! Run from the repo root:
//!   cargo run --example gen_macos_icons
//!
//! Writes:
//!   resources/macos/tray-icon-on.png   — green CC, captions active
//!   resources/macos/tray-icon-off.png  — dimmed green CC + strikethrough

fn write_png(path: &str, rgba: &[u8], size: u32) {
    let file = std::fs::File::create(path).expect("create file");
    let mut enc = png::Encoder::new(file, size, size);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("write header")
        .write_image_data(rgba)
        .expect("write pixels");
    println!("wrote {path}");
}

fn main() {
    let on  = subtidal::tray::icon::render_cc_icon_rgba(80, 200, 80, 1.0,  false);
    let off = subtidal::tray::icon::render_cc_icon_rgba(80, 200, 80, 0.35, true);
    write_png("resources/macos/tray-icon-on.png",  &on,  64);
    write_png("resources/macos/tray-icon-off.png", &off, 64);
}
