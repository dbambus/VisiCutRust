use resvg::{tiny_skia, usvg};

pub struct Preview {
    pub width_mm: f32,
    pub height_mm: f32,
    pub image: eframe::egui::ColorImage,
}

pub fn render(source: &str) -> Result<Preview, String> {
    if source.len() > 20 * 1024 * 1024 {
        return Err("SVG ist größer als 20 MB".into());
    }
    // Embedded projects must remain portable: external resources are not resolved.
    let mut options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(source, &options)
        .map_err(|e| format!("SVG konnte nicht gelesen werden: {e}"))?;
    let size = tree.size();
    let scale = (1600.0 / size.width().max(size.height())).min(2.0);
    let width = (size.width() * scale).ceil().max(1.0) as u32;
    let height = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("Vorschau zu groß")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia stores premultiplied RGBA; convert before handing pixels to egui.
    let mut pixels = pixmap.take();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    Ok(Preview {
        width_mm: size.width() * 25.4 / 96.0,
        height_mm: size.height() * 25.4 / 96.0,
        image: eframe::egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            &pixels,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn respects_physical_svg_dimensions() {
        let preview = render(include_str!("../examples/demo.svg")).unwrap();
        assert!((preview.width_mm - 100.0).abs() < 0.01);
        assert!((preview.height_mm - 60.0).abs() < 0.01);
        assert!(preview.image.pixels.iter().any(|p| p.a() > 0));
    }
    #[test]
    fn rejects_malformed_svg() {
        assert!(render("not an SVG").is_err());
    }
}
