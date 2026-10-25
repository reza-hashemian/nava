//! Renders ui/icons/nava.svg to PNGs for packaging: `cargo run --example render_icon`
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let svg = std::fs::read("ui/icons/nava.svg")?;
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())?;
    for size in [32u32, 64, 128, 256, 512] {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).ok_or("bad size")?;
        let scale = size as f32 / tree.size().width();
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        pixmap.save_png(format!("ui/icons/nava-{size}.png"))?;
    }
    Ok(())
}
