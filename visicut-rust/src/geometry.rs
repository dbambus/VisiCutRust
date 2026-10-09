use crate::project::Project;
use resvg::{tiny_skia, usvg};

pub type Point = [f32; 2];
pub type Contour = Vec<Point>;

pub fn contours(project: &Project) -> Result<Vec<Contour>, String> {
    let document = usvg::roxmltree::Document::parse(&project.svg).map_err(|e| e.to_string())?;
    for node in document.descendants().filter(|node| node.is_element()) {
        if matches!(node.tag_name().name(), "text" | "image")
            && !node.ancestors().any(|n| {
                matches!(
                    n.tag_name().name(),
                    "defs" | "symbol" | "clipPath" | "mask" | "pattern" | "marker"
                )
            })
        {
            return Err(
                "SVG enthält Text oder Bilder; zuerst in Pfade umwandeln oder Gravieren wählen"
                    .into(),
            );
        }
    }
    let mut options = usvg::Options::default();
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(&project.svg, &options).map_err(|e| e.to_string())?;
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
            usvg::Node::Text(_) => return Err("Text vor dem Schneiden in Pfade umwandeln".into()),
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
    fn does_not_silently_drop_unconverted_text() {
        let project = Project { svg: r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60"><text x="10" y="20">Job</text><rect width="10" height="10"/></svg>"#.into(), ..Default::default() };
        assert!(contours(&project).is_err());
    }
}
