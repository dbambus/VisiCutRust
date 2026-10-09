use crate::project::Project;
use resvg::{tiny_skia, usvg};
use std::sync::{Arc, Mutex};

pub type Point = [f32; 2];
pub type Contour = Vec<Point>;

pub fn contours(project: &Project) -> Result<Vec<Contour>, String> {
    contours_with_fonts(project, crate::svg::fonts())
}

fn contours_with_fonts(
    project: &Project,
    fonts: Arc<usvg::fontdb::Database>,
) -> Result<Vec<Contour>, String> {
    let document = usvg::roxmltree::Document::parse(&project.svg).map_err(|e| e.to_string())?;
    for node in document.descendants().filter(|node| node.is_element()) {
        if node.tag_name().name() == "image"
            && !node.ancestors().any(|n| {
                matches!(
                    n.tag_name().name(),
                    "defs" | "symbol" | "clipPath" | "mask" | "pattern" | "marker"
                )
            })
        {
            return Err(
                "SVG enthält Rasterbilder; zuerst in Pfade umwandeln oder Gravieren wählen".into(),
            );
        }
    }
    // Text is cut along its glyph outlines, like the Java importer does. Glyphs
    // usvg cannot place would otherwise vanish silently, so they are reported.
    let missing = Mutex::new(Vec::new());
    let options = usvg::Options {
        fontdb: fonts,
        font_resolver: outline_fonts(&missing),
        ..crate::svg::options()
    };
    let mut tree = usvg::Tree::from_str(&project.svg, &options).map_err(|e| e.to_string())?;
    if let Some(problem) = missing.lock().map_err(|e| e.to_string())?.first() {
        return Err(problem.clone());
    }
    if contains_text(tree.root()) {
        // usvg keeps glyph outlines relative to their <text> element (the flattened
        // paths carry an identity transform); writing the tree back out places them
        // under their ancestors' transforms like ordinary paths.
        let flattened = tree.to_string(&usvg::WriteOptions::default());
        tree = usvg::Tree::from_str(&flattened, &options).map_err(|e| e.to_string())?;
    }
    let mut result = Vec::new();
    visit(tree.root(), project, tree.size(), &mut result)?;
    if result.is_empty() {
        return Err("Keine schneidbaren Vektorpfade in der SVG".into());
    }
    if result.iter().map(Vec::len).sum::<usize>() > 1_000_000 {
        return Err("Zu viele Vektorpunkte".into());
    }
    Ok(result)
}

fn outline_fonts(missing: &Mutex<Vec<String>>) -> usvg::FontResolver<'_> {
    let select = usvg::FontResolver::default_font_selector();
    let fallback = usvg::FontResolver::default_fallback_selector();
    let report = move |problem: String| {
        if let Ok(mut list) = missing.lock() {
            list.push(problem);
        }
    };
    usvg::FontResolver {
        select_font: Box::new(move |font, db| {
            let id = select(font, db);
            if id.is_none() {
                let families: Vec<String> = font.families().iter().map(|f| f.to_string()).collect();
                report(format!(
                    "Schriftart für Text nicht gefunden ({}); Schriftart installieren oder Text in Pfade umwandeln",
                    families.join(", ")
                ));
            }
            id
        }),
        select_fallback: Box::new(move |c, used, db| {
            let id = fallback(c, used, db);
            let invisible = c.is_whitespace()
                || c.is_control()
                || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}');
            if id.is_none() && !invisible {
                report(format!(
                    "Zeichen „{c}“ (U+{:04X}) ist in keiner Schriftart enthalten; Schriftart installieren oder Text in Pfade umwandeln",
                    c as u32
                ));
            }
            id
        }),
    }
}

fn contains_text(group: &usvg::Group) -> bool {
    group.children().iter().any(|node| match node {
        usvg::Node::Text(_) => true,
        usvg::Node::Group(group) => contains_text(group),
        _ => false,
    })
}

fn visit(
    group: &usvg::Group,
    project: &Project,
    size: usvg::Size,
    result: &mut Vec<Contour>,
) -> Result<(), String> {
    if group.clip_path().is_some() || group.mask().is_some() || !group.filters().is_empty() {
        return Err("SVG-Clipping, Masken oder Filter zuerst in echte Pfade umwandeln".into());
    }
    if group.opacity().get() == 0.0 {
        return Ok(());
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(group) => visit(group, project, size, result)?,
            usvg::Node::Path(path) if path.is_visible() => {
                if path.fill().is_none_or(|fill| fill.opacity().get() == 0.0)
                    && path
                        .stroke()
                        .is_none_or(|stroke| stroke.opacity().get() == 0.0)
                {
                    continue;
                }
                let map = |point: tiny_skia::Point| {
                    let mut point = point;
                    path.abs_transform().map_point(&mut point);
                    [
                        project.x_mm + point.x * project.width_mm / size.width(),
                        project.y_mm + point.y * project.height_mm / size.height(),
                    ]
                };
                let mut contour = Vec::new();
                let mut current = [0.0; 2];
                let mut start = [0.0; 2];
                for segment in path.data().segments() {
                    match segment {
                        tiny_skia::PathSegment::MoveTo(point) => {
                            if contour.len() > 1 {
                                result.push(std::mem::take(&mut contour));
                            }
                            contour.clear();
                            current = map(point);
                            start = current;
                            contour.push(current);
                        }
                        tiny_skia::PathSegment::LineTo(point) => {
                            current = map(point);
                            contour.push(current);
                        }
                        tiny_skia::PathSegment::QuadTo(a, end) => {
                            let a = map(a);
                            let end = map(end);
                            let c1 = mix(current, a, 2.0 / 3.0);
                            let c2 = mix(end, a, 2.0 / 3.0);
                            flatten([current, c1, c2, end], 0, &mut contour);
                            current = end;
                        }
                        tiny_skia::PathSegment::CubicTo(a, b, end) => {
                            let end = map(end);
                            flatten([current, map(a), map(b), end], 0, &mut contour);
                            current = end;
                        }
                        tiny_skia::PathSegment::Close => {
                            contour.push(start);
                            current = start;
                        }
                    }
                }
                if contour.len() > 1 {
                    result.push(contour);
                }
            }
            usvg::Node::Image(_) => {
                return Err(
                    "Schneiden unterstützt keine Rasterbilder; bitte Gravieren wählen".into(),
                );
            }
            // `contours` re-imports text as paths before visiting.
            usvg::Node::Text(_) => {
                return Err("Text konnte nicht in Pfade umgewandelt werden".into());
            }
            _ => {}
        }
    }
    Ok(())
}

fn mix(a: Point, b: Point, t: f32) -> Point {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn flatten(p: [Point; 4], depth: u32, out: &mut Contour) {
    // Control-polygon excess catches loops as well as curves along a straight chord.
    let distance = |a: Point, b: Point| (a[0] - b[0]).hypot(a[1] - b[1]);
    let excess =
        distance(p[0], p[1]) + distance(p[1], p[2]) + distance(p[2], p[3]) - distance(p[0], p[3]);
    let chord_distance = |a: Point| {
        let dx = p[3][0] - p[0][0];
        let dy = p[3][1] - p[0][1];
        ((a[0] - p[0][0]) * dy - (a[1] - p[0][1]) * dx).abs() / dx.hypot(dy).max(1e-9)
    };
    if depth >= 16 || (excess <= 0.025 && chord_distance(p[1]).max(chord_distance(p[2])) <= 0.025) {
        out.push(p[3]);
        return;
    }
    let a = mix(p[0], p[1], 0.5);
    let b = mix(p[1], p[2], 0.5);
    let c = mix(p[2], p[3], 0.5);
    let d = mix(a, b, 0.5);
    let e = mix(b, c, 0.5);
    let f = mix(d, e, 0.5);
    flatten([p[0], a, d, f], depth + 1, out);
    flatten([f, e, c, p[3]], depth + 1, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transforms_curves_and_closes_shapes() {
        let project = Project {
            svg: include_str!("../examples/demo.svg").into(),
            ..Default::default()
        };
        let paths = contours(&project).unwrap();
        assert_eq!(paths.len(), 3);
        assert!(paths[1].len() > 30);
        assert_eq!(paths[1].first(), paths[1].last());
        assert!(paths.iter().flatten().all(|p| p[0] >= 10.0 && p[1] >= 10.0));
    }
    #[test]
    fn rejects_clipped_cut_paths() {
        let project = Project { svg: r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><clipPath id="c"><rect width="10" height="10"/></clipPath></defs><g clip-path="url(#c)"><rect width="100" height="100"/></g></svg>"#.into(), ..Default::default() };
        assert!(contours(&project).is_err());
    }

    #[test]
    fn applies_viewbox_and_parent_transform() {
        let project = Project { svg: r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="60mm" viewBox="0 0 100 60"><g transform="translate(20 5)"><path d="M0 0L10 10" fill="none" stroke="black"/></g></svg>"#.into(), ..Default::default() };
        let paths = contours(&project).unwrap();
        assert!((paths[0][0][0] - 30.0).abs() < 0.001);
        assert!((paths[0][0][1] - 15.0).abs() < 0.001);
        assert!((paths[0][1][0] - 40.0).abs() < 0.001);
    }

    #[test]
    fn cuts_text_along_glyph_outlines_in_mm() {
        let project = Project { svg: r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="60mm" viewBox="0 0 100 60"><g transform="translate(20 30) scale(2)"><text x="0" y="0" font-family="sans-serif" font-size="10" fill="none" stroke="red">H</text></g><rect x="90" y="50" width="5" height="5"/></svg>"#.into(), x_mm: 0.0, y_mm: 0.0, width_mm: 100.0, height_mm: 60.0, ..Default::default() };
        let paths = contours(&project).unwrap();
        let glyph: Vec<Point> = paths
            .iter()
            .flatten()
            .copied()
            .filter(|p| p[0] < 80.0)
            .collect();
        assert!(!glyph.is_empty());
        let (min_x, max_x) = glyph
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
        let (min_y, max_y) = glyph
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
        // A 20 mm "H" sits on the baseline at y = 30 mm, starting at x = 20 mm.
        assert!(min_x >= 20.0 && max_x < 40.0, "x {min_x}..{max_x}");
        assert!(max_y <= 30.5 && max_y > 29.0, "y max {max_y}");
        assert!(min_y > 10.0 && min_y < 25.0, "y min {min_y}");
        assert!(paths.iter().any(|p| p.iter().all(|q| q[0] >= 90.0)));
    }

    #[test]
    fn reports_text_without_font() {
        let project = Project { svg: r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60"><text x="10" y="20">Job</text><rect width="10" height="10"/></svg>"#.into(), ..Default::default() };
        let error =
            contours_with_fonts(&project, Arc::new(usvg::fontdb::Database::new())).unwrap_err();
        assert!(error.contains("Schriftart"), "{error}");
    }

    #[test]
    fn reports_characters_missing_from_every_font() {
        let mut fonts = usvg::fontdb::Database::new();
        fonts.load_font_data(epaint_default_fonts::UBUNTU_LIGHT.to_vec());
        let project = Project { svg: "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"60\"><text x=\"10\" y=\"20\" font-family=\"Ubuntu\">A \u{10FFFD}</text></svg>".into(), ..Default::default() };
        let error = contours_with_fonts(&project, Arc::new(fonts)).unwrap_err();
        assert!(error.contains("U+10FFFD"), "{error}");
    }

    #[test]
    fn bundled_fonts_cover_generic_families() {
        let fonts = crate::svg::fonts();
        for family in [
            usvg::fontdb::Family::Serif,
            usvg::fontdb::Family::SansSerif,
            usvg::fontdb::Family::Monospace,
            usvg::fontdb::Family::Cursive,
            usvg::fontdb::Family::Fantasy,
        ] {
            let query = usvg::fontdb::Query {
                families: &[family],
                ..Default::default()
            };
            assert!(fonts.query(&query).is_some(), "{family:?}");
        }
    }
}
