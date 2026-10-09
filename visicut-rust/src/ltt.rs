// Protocol port from LibLaserCut LaserToolsTechnicsCutter, LGPL-3.0-or-later.
// Original driver: Maximilian Gaukler; portions by Thomas Oster.
// Provenance and deliberate limitations: ../PROTOCOL.md.
use crate::{
    geometry,
    project::{Operation, Project},
    timeline::{MotionKind, Program, Timeline},
};
use resvg::{tiny_skia, usvg};
use std::{
    io::Write,
    net::{Shutdown, TcpStream, ToSocketAddrs},
    time::Duration,
};

const MACHINE_DPI: f64 = 4000.0;
const RASTER_DPI: f64 = 500.0;
const BED_HEIGHT: f64 = 600.0;
const BED_WIDTH: f64 = 1000.0;

fn raw(mm: f64) -> u32 {
    (mm * MACHINE_DPI / 25.4).round() as u32
}
fn px(mm: f64) -> i32 {
    (mm * RASTER_DPI / 25.4) as i32
}
fn word(out: &mut Vec<u8>, value: u16) {
    out.extend(value.to_be_bytes());
}
fn dword(out: &mut Vec<u8>, value: u32) {
    out.extend(value.to_be_bytes());
}
fn pair(out: &mut Vec<u8>, x: i32, y: i32) {
    dword(out, (x * 8) as u32);
    dword(
        out,
        ((BED_HEIGHT * MACHINE_DPI / 25.4) as i32 - y * 8) as u32,
    );
}

#[derive(serde::Serialize)]
pub struct PreparedStep {
    pub operation: Operation,
    pub name: String,
    pub description: String,
    pub estimated_seconds: f64,
    pub power_percent: f32,
    pub speed_percent: f32,
    pub passes: u32,
}

#[derive(serde::Serialize)]
pub struct OutputJob {
    pub name: String,
    pub operation: Operation,
    pub bytes: Vec<u8>,
}

pub struct PreparedJob {
    pub jobs: Vec<OutputJob>,
    pub description: String,
    pub estimated_seconds: f64,
    pub preview_png: Vec<u8>,
    pub steps: Vec<PreparedStep>,
    pub timeline: Timeline,
}

fn effective_speed(speed: f32) -> f64 {
    (speed * 10.0).clamp(1.0, 1000.0) as u16 as f64 / 1000.0
}

fn overscan(project: &Project) -> f32 {
    if project.operation == Operation::Engrave {
        3.5_f32.max(35.0 * (project.speed_percent / 100.0).powi(2))
    } else {
        0.0
    }
}

pub fn prepare(project: &Project) -> Result<PreparedJob, String> {
    project.validate()?;
    if (project.bed_width_mm - BED_WIDTH as f32).abs() > 0.01
        || (project.bed_height_mm - BED_HEIGHT as f32).abs() > 0.01
    {
        return Err("Das FAU-LTT-Profil benötigt ein Arbeitsbett von 1000 × 600 mm".into());
    }
    let mut parts = Vec::new();
    if project.steps.is_empty() {
        parts.push(project.clone());
    } else {
        for step in &project.steps {
            if step.objects.is_empty() {
                continue;
            }
            let mut part = project.clone();
            part.steps.clear();
            part.svg = crate::selection::filter(&project.svg, &step.objects)?;
            part.operation = step.operation;
            part.power_percent = step.power_percent;
            part.speed_percent = step.speed_percent;
            part.passes = step.passes;
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return Err("Keine Objekte zur Bearbeitung ausgewählt".into());
    }
    // Send finishing operations first, cutting last. Each gets its own LTT file.
    parts.sort_by_key(|p| p.operation.order());
    let mut jobs = Vec::new();
    let mut total_bytes: usize = 0;
    let scale = 1600.0 / project.width_mm.max(project.height_mm);
    let mut preview = tiny_skia::Pixmap::new(
        (project.width_mm * scale).ceil().max(1.0) as u32,
        (project.height_mm * scale).ceil().max(1.0) as u32,
    )
    .ok_or("Vorschau konnte nicht erstellt werden")?;
    let mut steps = Vec::new();
    let mut timeline = Timeline::default();
    for part in &parts {
        let name = device_job_name(part.operation, &project.name);
        let mut out = header(part, overscan(part), &name);
        steps.push(append_part(&mut out, part, &mut preview, &mut timeline)?);
        finish(&mut out)?;
        total_bytes = total_bytes.saturating_add(out.len());
        if total_bytes > 256 * 1024 * 1024 {
            return Err("Gesamtauftrag ist größer als 256 MB".into());
        }
        jobs.push(OutputJob {
            name,
            operation: part.operation,
            bytes: out,
        });
    }
    Ok(PreparedJob {
        jobs,
        description: steps
            .iter()
            .map(|s| s.description.as_str())
            .collect::<Vec<_>>()
            .join(" · "),
        estimated_seconds: timeline.duration_seconds,
        preview_png: preview.encode_png().map_err(|e| e.to_string())?,
        steps,
        timeline,
    })
}

fn device_job_name(operation: Operation, name: &str) -> String {
    format!("{}_{}", operation.prefix(), name)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(15)
        .collect()
}

fn finish(out: &mut Vec<u8>) -> Result<(), String> {
    out.extend([0x1b, 0x42, 0x59, 0x45]);
    let checksum = out
        .iter()
        .fold(0u16, |sum, byte| sum.wrapping_add(*byte as u16));
    let length = u32::try_from(out.len() + 6).map_err(|_| "Job ist zu groß")?;
    word(out, checksum);
    dword(out, length);
    Ok(())
}

fn append_part(
    out: &mut Vec<u8>,
    project: &Project,
    preview: &mut tiny_skia::Pixmap,
    timeline: &mut Timeline,
) -> Result<PreparedStep, String> {
    if project.power_percent < 0.1 {
        return Err("Leistung für einen LTT-Job muss mindestens 0,1 % sein".into());
    }
    let speed = project.speed_percent;
    let overscan = overscan(project);
    let description;
    let mut program = Program::new(project.operation);
    match project.operation {
        Operation::Cut | Operation::Mark => {
            let paths = geometry::contours(project)?;
            let estimated = paths
                .iter()
                .map(Vec::len)
                .sum::<usize>()
                .saturating_mul(project.passes as usize)
                .saturating_mul(12);
            if estimated > 256 * 1024 * 1024 {
                return Err("Vektorjob ist größer als 256 MB".into());
            }
            draw_contours(preview, project, &paths);
            for point in paths.iter().flatten() {
                if !point[0].is_finite()
                    || !point[1].is_finite()
                    || point[0] < 0.0
                    || point[1] < 0.0
                    || point[0] > BED_WIDTH as f32
                    || point[1] > BED_HEIGHT as f32
                {
                    return Err(
                        "SVG-Pfad liegt außerhalb des tatsächlichen LTT-Arbeitsbetts".into(),
                    );
                }
            }
            out.extend([0x1b, 0x56]);
            out.extend([0x1b, 0x45, 0, 0, 0, 0, 0, 0, 0]);
            out.extend([0x1b, 0x4e, 1]);
            out.extend(b"PS");
            out.extend([0x1b, 0x50, 0, 4]);
            settings(out, project.power_percent, speed);
            for pass in 0..project.passes {
                for path in &paths {
                    // Quantize at 500 DPI before delta subtraction, as in LibLaserCut.
                    let first = path[0];
                    let mut current = [px(first[0] as f64), px(first[1] as f64)];
                    if pass == 0 {
                        program.travel(mm_point(current));
                    }
                    out.extend(b"PA");
                    pair(out, current[0], current[1]);
                    out.extend(b"PD");
                    for point in path.iter().skip(1) {
                        let next = [px(point[0] as f64), px(point[1] as f64)];
                        if next == current {
                            continue;
                        }
                        if pass == 0 {
                            program.line(
                                if project.operation == Operation::Mark {
                                    MotionKind::Mark
                                } else {
                                    MotionKind::Cut
                                },
                                mm_point(next),
                                338.677 * effective_speed(speed),
                            );
                        }
                        out.extend(b"PR");
                        out.extend(((next[0] - current[0]) * 8).to_be_bytes());
                        out.extend((-(next[1] - current[1]) * 8).to_be_bytes());
                        current = next;
                    }
                    out.extend(b"PU");
                }
            }
            description = format!(
                "{} Vektorpfade · {} Durchgänge",
                paths.len(),
                project.passes
            );
        }
        Operation::Engrave => {
            out.extend([0x1b, 0x4e, 0]);
            settings(out, project.power_percent, speed);
            let bitmap = raster(project)?;
            let lines = raster_code(out, project, &bitmap, speed, overscan, &mut program)?;
            draw_raster(preview, &bitmap);
            program.raster_preview_png = preview.encode_png().map_err(|e| e.to_string())?;
            program.raster_bounds_mm = Some([
                px(project.x_mm as f64) as f64 * 25.4 / RASTER_DPI,
                px(project.y_mm as f64) as f64 * 25.4 / RASTER_DPI,
                bitmap.width() as f64 * 25.4 / RASTER_DPI,
                bitmap.height() as f64 * 25.4 / RASTER_DPI,
            ]);
            if lines == 0 {
                return Err("SVG enthält keine dunklen Gravurpixel".into());
            }
            description = format!(
                "{lines} Gravurzeilen · 500 DPI · {} Durchgänge",
                project.passes
            );
        }
    }
    Ok(PreparedStep {
        operation: project.operation,
        name: device_job_name(project.operation, &project.name),
        description,
        estimated_seconds: timeline.append(program, project.passes),
        power_percent: project.power_percent,
        speed_percent: project.speed_percent,
        passes: project.passes,
    })
}

fn mm_point(point: [i32; 2]) -> [f64; 2] {
    point.map(|p| p as f64 * 25.4 / RASTER_DPI)
}

fn dark(pixel: &[u8]) -> bool {
    (pixel[0] as u32 * 2126 + pixel[1] as u32 * 7152 + pixel[2] as u32 * 722) / 10000 < 128
}

fn draw_raster(preview: &mut tiny_skia::Pixmap, bitmap: &tiny_skia::Pixmap) {
    let mut mask = bitmap.clone();
    for pixel in mask.data_mut().chunks_exact_mut(4) {
        let color = if dark(pixel) {
            [35, 113, 210, 255]
        } else {
            [0, 0, 0, 0]
        };
        pixel.copy_from_slice(&color);
    }
    preview.draw_pixmap(
        0,
        0,
        mask.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::from_scale(
            preview.width() as f32 / mask.width() as f32,
            preview.height() as f32 / mask.height() as f32,
        ),
        None,
    );
}

fn draw_contours(preview: &mut tiny_skia::Pixmap, project: &Project, paths: &[geometry::Contour]) {
    let sx = preview.width() as f32 / project.width_mm;
    let sy = preview.height() as f32 / project.height_mm;
    let mut paint = tiny_skia::Paint::default();
    if project.operation == Operation::Mark {
        paint.set_color_rgba8(142, 68, 210, 255);
    } else {
        paint.set_color_rgba8(220, 52, 52, 255);
    }
    paint.anti_alias = true;
    for path in paths {
        let mut builder = tiny_skia::PathBuilder::new();
        for (i, point) in path.iter().enumerate() {
            let x = (px(point[0] as f64) as f32 * 25.4 / RASTER_DPI as f32 - project.x_mm) * sx;
            let y = (px(point[1] as f64) as f32 * 25.4 / RASTER_DPI as f32 - project.y_mm) * sy;
            if i == 0 {
                builder.move_to(x, y);
            } else {
                builder.line_to(x, y);
            }
        }
        if let Some(path) = builder.finish() {
            preview.stroke_path(
                &path,
                &paint,
                &tiny_skia::Stroke {
                    width: 1.6,
                    ..Default::default()
                },
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
}

fn settings(out: &mut Vec<u8>, power: f32, speed: f32) {
    out.extend([0x1b, 0x53]);
    word(out, (speed * 10.0).clamp(1.0, 1000.0) as u16);
    out.extend([0x1b, 0x4a]);
    word(out, (power * 10.0).clamp(1.0, 1000.0) as u16);
}

fn header(project: &Project, overscan: f32, name: &str) -> Vec<u8> {
    let mut out = b"LTT\x1bv\x01\x01\x02\x1bF".to_vec();
    // Prefix is part of the actual controller name, within its 15-byte limit.
    out.push(name.len() as u8);
    out.extend(name.as_bytes());
    out.extend([0x1b, 0x61, 0]); // temporary reference point off
    out.extend([0x1b, 0x4d, 0]); // XY, one bit per raster pixel
    out.extend([0x1b, 0x6c]);
    for margin in [0.0, overscan] {
        let xmin = (project.x_mm - margin).max(0.0) as f64;
        let xmax = (project.x_mm + project.width_mm + margin).min(BED_WIDTH as f32) as f64;
        let ymin = project.y_mm as f64;
        let ymax = (project.y_mm + project.height_mm) as f64;
        dword(&mut out, raw(xmin));
        dword(&mut out, raw(ymin));
        dword(&mut out, raw(xmax) - raw(xmin));
        dword(&mut out, raw(ymax) - raw(ymin));
    }
    out.extend([0x1b, 0x6e, 0, 0, 0x5d, 0xcf, 0, 0, 0x69, 0x56]);
    out.extend([0x1b, 0x4f, 0]); // no autorun
    out.extend([0x1b, 0x51, 0, 0]);
    out.extend([0x1b, 0x44, 8]); // 4000 / 8 = 500 DPI
    out.extend([0x1b, 0x52]);
    word(&mut out, 4200);
    out.extend([0x1b, 0x43, 0xc0]);
    out.extend([0x1b, 0x54]);
    out.extend((0..=15).map(|i| i * 0x11));
    out
}

fn raster(project: &Project) -> Result<tiny_skia::Pixmap, String> {
    let width = px(project.width_mm as f64).max(1) as u32;
    let height = px(project.height_mm as f64).max(1) as u32;
    if width as u64 * height as u64 > 40_000_000 {
        return Err("Gravur ist größer als 40 Millionen Pixel; Motiv verkleinern".into());
    }
    let mut options = usvg::Options::default();
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(&project.svg, &options).map_err(|e| e.to_string())?;
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or("Gravur konnte nicht gerendert werden")?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(
            width as f32 / tree.size().width(),
            height as f32 / tree.size().height(),
        ),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

fn raster_code(
    out: &mut Vec<u8>,
    project: &Project,
    bitmap: &tiny_skia::Pixmap,
    speed: f32,
    overscan: f32,
    program: &mut Program,
) -> Result<usize, String> {
    let offset_table = [
        -2.0, -4.0, -7.0, -10.0, -13.0, -15.0, -17.0, -19.0, -21.0, -23.0,
    ];
    let index = (speed / 10.0 - 1.0).clamp(0.0, 9.0);
    let low = index.floor() as usize;
    let shift_raw =
        offset_table[low] + (offset_table[(low + 1).min(9)] - offset_table[low]) * index.fract();
    let shift = (-shift_raw * RASTER_DPI as f32 / MACHINE_DPI as f32) as usize;
    let spare = (shift_raw * RASTER_DPI as f32 / MACHINE_DPI as f32)
        .abs()
        .ceil() as i32;
    let origin_x = px(project.x_mm as f64);
    let origin_y = px(project.y_mm as f64);
    let max_x = px(BED_WIDTH);
    let overscan = px(overscan as f64);
    let mut count = 0;
    let mut encoded = Vec::new();
    for y in 0..bitmap.height() {
        let mut row = vec![0u8; (bitmap.width() as usize).div_ceil(8)];
        for x in 0..bitmap.width() {
            let i = (y as usize * bitmap.width() as usize + x as usize) * 4;
            let pixel = &bitmap.data()[i..i + 4];
            if dark(pixel) {
                row[x as usize / 8] |= 0x80 >> (x % 8);
            }
        }
        let Some(first) = row.iter().position(|b| *b != 0) else {
            continue;
        };
        let last = row.iter().rposition(|b| *b != 0).unwrap();
        let mut start_x = origin_x + first as i32 * 8;
        let mut row = row[first..=last].to_vec();
        let left_bytes = ((overscan + 7) / 8).min(((start_x - spare) / 8).max(0)) as usize;
        start_x -= left_bytes as i32 * 8;
        let mut padded = vec![0; left_bytes];
        padded.append(&mut row);
        let right_bytes = ((overscan + 7) / 8)
            .min(((max_x - spare - 8 - start_x - padded.len() as i32 * 8) / 8).max(0))
            as usize;
        padded.resize(padded.len() + right_bytes, 0);
        if start_x < spare || start_x + padded.len() as i32 * 8 > max_x - spare {
            return Err(
                "Gravur zu nahe am Bettrand; bitte Motiv weiter nach innen verschieben".into(),
            );
        }
        let start = [
            start_x as f64 * 25.4 / RASTER_DPI,
            (origin_y + y as i32) as f64 * 25.4 / RASTER_DPI,
        ];
        let distance = padded.len() as f64 * 8.0 * 25.4 / RASTER_DPI;
        program.travel(start);
        program.dwell(0.1);
        program.line(
            MotionKind::Raster,
            [start[0] + distance, start[1]],
            338.677 * 6.4 * effective_speed(speed),
        );
        left_shift(&mut padded, shift);
        let compressed = compress(&padded);
        encoded.extend([0x1b, 0x30]); // unidirectional: every line left to right
        dword(&mut encoded, compressed.len() as u32 + 8);
        pair(&mut encoded, start_x, origin_y + y as i32);
        encoded.extend(compressed);
        count += 1;
    }
    if encoded.len().saturating_mul(project.passes as usize) > 256 * 1024 * 1024 {
        return Err("Gravurjob ist größer als 256 MB".into());
    }
    for _ in 0..project.passes {
        out.extend(&encoded);
    }
    Ok(count)
}

fn left_shift(bytes: &mut [u8], bits: usize) {
    if bits == 0 {
        return;
    }
    for i in 0..bytes.len() {
        bytes[i] = (bytes[i] << bits) | (bytes.get(i + 1).copied().unwrap_or(0) >> (8 - bits));
    }
}

fn compress(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let value = bytes[i];
        let mut run = 1;
        while i + run < bytes.len() && bytes[i + run] == value && run < 63 {
            run += 1;
        }
        if run > 1 || value >= 0xc0 {
            out.push(0xc0 + run as u8);
        }
        out.push(value);
        i += run;
    }
    out
}

pub fn transmit_jobs(host: &str, port: u16, jobs: &[OutputJob]) -> Result<Vec<String>, String> {
    transmit_jobs_with(jobs, |bytes| transmit(host, port, bytes))
}

fn transmit_jobs_with(
    jobs: &[OutputJob],
    mut send: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let mut sent = Vec::new();
    for job in jobs {
        if let Err(error) = send(&job.bytes) {
            return Err(format!(
                "Übertragung von {} fehlgeschlagen: {}\nBereits übertragen: {}. Dieser Job kann teilweise am Gerät liegen. Weitere Aufträge wurden nicht gesendet; vor erneutem Senden Gerätespeicher prüfen.",
                job.name,
                error,
                if sent.is_empty() {
                    "keine".into()
                } else {
                    sent.join(", ")
                }
            ));
        }
        sent.push(job.name.clone());
    }
    Ok(sent)
}

pub fn transmit(host: &str, port: u16, bytes: &[u8]) -> Result<(), String> {
    if host.trim().is_empty() || port == 0 {
        return Err("Hostname und Port müssen gesetzt sein".into());
    }
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("Hostname nicht erreichbar: {e}"))?
        .collect();
    let mut connection = None;
    for address in addresses {
        if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_secs(3)) {
            connection = Some(stream);
            break;
        }
    }
    let mut stream = connection.ok_or("Keine Verbindung zum LTT möglich")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(bytes)
        .map_err(|e| format!("Übertragung fehlgeschlagen; Teiljob könnte am Gerät liegen: {e}"))?;
    stream.flush().map_err(|e| e.to_string())?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Project {
        Project {
            svg: include_str!("../examples/demo.svg").into(),
            ..Default::default()
        }
    }
    fn mixed() -> Project {
        use crate::project::JobStep;
        Project {
            width_mm: 30.0, height_mm: 20.0,
            svg: r#"<svg xmlns="http://www.w3.org/2000/svg" width="30mm" height="20mm" viewBox="0 0 30 20"><rect id="cut" x="1" y="1" width="28" height="18" fill="none" stroke="red"/><rect id="engrave" x="5" y="5" width="8" height="5" fill="black"/><text x="15" y="10">ignored</text></svg>"#.into(),
            steps: vec![
                JobStep { operation: Operation::Cut, objects: vec![0], power_percent: 50.0, speed_percent: 8.0, passes: 2 },
                JobStep { operation: Operation::Engrave, objects: vec![1], power_percent: 20.0, speed_percent: 70.0, passes: 1 },
            ], ..Default::default()
        }
    }

    #[test]
    fn mixed_selection_encodes_separate_jobs_engrave_before_cut_with_separate_settings() {
        let p = mixed();
        let job = prepare(&p).unwrap();
        assert_eq!(job.jobs.len(), 2);
        assert_eq!(job.jobs[0].operation, Operation::Engrave);
        assert_eq!(job.jobs[1].operation, Operation::Cut);
        assert!(job.jobs[0].bytes.windows(2).any(|b| b == [0x1b, 0x30]));
        assert!(!job.jobs[0].bytes.windows(2).any(|b| b == [0x1b, 0x56]));
        assert!(job.jobs[1].bytes.windows(2).any(|b| b == [0x1b, 0x56]));
        assert!(job.steps[1].description.contains("1 Vektorpfade"));
        assert!(
            job.jobs[0]
                .bytes
                .windows(4)
                .any(|b| b == [0x1b, 0x53, 2, 188])
        );
        assert!(
            job.jobs[1]
                .bytes
                .windows(4)
                .any(|b| b == [0x1b, 0x53, 0, 80])
        );
        for output in &job.jobs {
            assert_framing(&output.bytes);
        }
        assert!(
            (job.estimated_seconds - job.steps.iter().map(|s| s.estimated_seconds).sum::<f64>())
                .abs()
                < 0.001
        );
        let image = tiny_skia::Pixmap::decode_png(&job.preview_png).unwrap();
        assert!(
            image
                .pixels()
                .iter()
                .any(|p| p.red() > 150 && p.blue() < 100)
        );
        assert!(
            image
                .pixels()
                .iter()
                .any(|p| p.blue() > 150 && p.red() < 100)
        );
        let loaded: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        let reloaded = prepare(&loaded).unwrap();
        for (a, b) in job.jobs.iter().zip(&reloaded.jobs) {
            assert_eq!(a.bytes, b.bytes);
        }
    }

    fn three_modes() -> Project {
        let mut p = mixed();
        p.name = "Muster mit sehr langem Namen äöü".into();
        p.svg = p.svg.replace(
            "<text x=\"15\" y=\"10\">ignored</text>",
            "<path d=\"M15 10L25 10\" stroke=\"black\"/>",
        );
        p.steps.push(crate::project::JobStep {
            operation: Operation::Mark,
            objects: vec![2],
            power_percent: 5.0,
            speed_percent: 60.0,
            passes: 2,
        });
        p
    }

    #[test]
    fn three_modes_have_distinct_prefixed_files_and_ordered_simulation() {
        let p = three_modes();
        let job = prepare(&p).unwrap();
        assert_eq!(job.jobs.len(), 3);
        for (i, prefix) in ["Engrav_", "Mark_", "Cut_"].iter().enumerate() {
            let output = &job.jobs[i];
            assert!(output.name.starts_with(prefix));
            assert!(output.name.len() <= 15);
            assert!(output.name.is_ascii());
            assert_eq!(output.bytes[10] as usize, output.name.len());
            assert_eq!(
                &output.bytes[11..11 + output.name.len()],
                output.name.as_bytes()
            );
            assert_framing(&output.bytes);
        }
        assert!(
            job.jobs[1]
                .bytes
                .windows(4)
                .any(|b| b == [0x1b, 0x4a, 0, 50])
        );
        assert_eq!(job.timeline.programs.len(), 3);
        assert_eq!(job.timeline.runs.len(), 5);
        assert_eq!(job.timeline.programs[1].operation, Operation::Mark);
        assert!(
            job.timeline.programs[1]
                .motions
                .iter()
                .any(|m| m.kind == MotionKind::Mark)
        );
        for run in &job.timeline.runs {
            assert!(run.start_seconds <= run.entry_end_seconds);
            assert!(run.entry_end_seconds <= run.end_seconds);
        }
        assert_eq!(
            job.estimated_seconds,
            job.timeline.runs.last().unwrap().end_seconds
        );
        let mark = job.timeline.programs[1]
            .motions
            .iter()
            .find(|m| m.kind == MotionKind::Mark)
            .unwrap();
        assert!((mark.to_mm[0] - mark.from_mm[0] - 10.0).abs() < 0.06);
        let saved: Project = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        let again = prepare(&saved).unwrap();
        for (a, b) in job.jobs.iter().zip(&again.jobs) {
            assert_eq!(a.bytes, b.bytes);
        }
        let mut mark_only = p.clone();
        mark_only.steps.retain(|s| s.operation == Operation::Mark);
        assert_eq!(prepare(&mark_only).unwrap().jobs.len(), 1);
        mark_only.steps[0].objects.clear();
        assert!(prepare(&mark_only).is_err());
    }

    #[test]
    fn batch_transfer_stops_on_error_and_reports_already_sent_names() {
        let job = prepare(&three_modes()).unwrap();
        let mut calls = 0;
        let error = transmit_jobs_with(&job.jobs, |_| {
            calls += 1;
            if calls == 2 {
                Err("Test-Verbindungsabbruch".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(calls, 2);
        assert!(error.contains(&job.jobs[0].name));
        assert!(error.contains(&job.jobs[1].name));
        assert!(!error.contains(&job.jobs[2].name));
        let sent = transmit_jobs_with(&job.jobs, |_| Ok(())).unwrap();
        assert_eq!(
            sent,
            job.jobs.iter().map(|j| j.name.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn tcp_batch_uses_three_connections_with_exact_framed_payloads() {
        use std::io::Read;
        let job = prepare(&three_modes()).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let receiver = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut payloads = Vec::new();
            while payloads.len() < 3 {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        let mut bytes = Vec::new();
                        stream.read_to_end(&mut bytes).unwrap();
                        payloads.push(bytes);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "Missing batch connection"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
            payloads
        });
        let sent = transmit_jobs("127.0.0.1", port, &job.jobs).unwrap();
        assert_eq!(sent.len(), 3);
        let received = receiver.join().unwrap();
        assert_eq!(received.len(), 3);
        for (bytes, output) in received.iter().zip(&job.jobs) {
            assert_eq!(*bytes, output.bytes);
        }
    }

    #[test]
    fn estimates_quantized_cut_distance_speed_and_passes() {
        let mut p = Project { x_mm: 0.0, y_mm: 0.0, width_mm: 100.0, height_mm: 10.0,
            speed_percent: 10.0,
            svg: r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="10mm" viewBox="0 0 100 10"><path d="M0 0L100 0" stroke="black"/></svg>"#.into(), ..Default::default() };
        let one = prepare(&p).unwrap().estimated_seconds;
        assert!((one - 100.0 / 33.8677).abs() < 0.003);
        p.speed_percent = 20.0;
        let faster = prepare(&p).unwrap().estimated_seconds;
        assert!((faster * 2.0 - one).abs() < 0.001);
        p.passes = 2;
        assert!(prepare(&p).unwrap().estimated_seconds > faster * 2.0); // return travel
    }

    #[test]
    fn raster_time_scales_with_passes_and_includes_line_overhead() {
        let mut p = mixed();
        p.steps.remove(0);
        let one = prepare(&p).unwrap();
        assert!(one.estimated_seconds > 5.0 / 25.4 * RASTER_DPI * 0.1);
        p.steps[0].passes = 3;
        let three = prepare(&p).unwrap();
        let program = &three.timeline.programs[0];
        let return_time = (program.end_mm[0] - program.start_mm[0])
            .hypot(program.end_mm[1] - program.start_mm[1])
            / 338.677;
        assert!(
            (three.estimated_seconds
                - one.estimated_seconds
                - 2.0 * (program.duration_seconds + return_time))
                .abs()
                < 0.001
        );
        assert_eq!(
            one.timeline.programs[0].motions.len(),
            program.motions.len()
        );
    }

    #[test]
    fn rejects_empty_duplicate_stale_and_invalid_step_selections() {
        let mut p = mixed();
        p.steps[0].objects.clear();
        p.steps[1].objects.clear();
        assert!(prepare(&p).err().unwrap().contains("Keine Objekte"));
        let mut p = mixed();
        p.steps[0].objects = vec![1];
        assert!(prepare(&p).is_err());
        let mut p = mixed();
        p.steps[0].objects = vec![1000];
        assert!(prepare(&p).is_err());
        let mut p = mixed();
        p.steps[1].speed_percent = 0.0;
        assert!(prepare(&p).is_err());
    }

    #[test]
    fn framing_checksum_length_and_no_autorun() {
        let prepared = prepare(&sample()).unwrap();
        assert_framing(&prepared.jobs[0].bytes);
    }

    fn assert_framing(bytes: &[u8]) {
        assert!(bytes.starts_with(b"LTT\x1bv\x01\x01\x02"));
        assert!(bytes.windows(3).any(|b| b == [0x1b, 0x4f, 0]));
        let n = bytes.len();
        assert_eq!(
            u32::from_be_bytes(bytes[n - 4..].try_into().unwrap()) as usize,
            n
        );
        assert_eq!(
            u16::from_be_bytes(bytes[n - 6..n - 4].try_into().unwrap()),
            bytes[..n - 6]
                .iter()
                .fold(0u16, |s, b| s.wrapping_add(*b as u16))
        );
        assert_eq!(&bytes[n - 10..n - 6], b"\x1bBYE");
    }
    #[test]
    fn coordinates_match_java_driver_conversion() {
        let mut bytes = Vec::new();
        pair(&mut bytes, 196, 393);
        // Java casts bedHeight*4000/25.4 to int, scales 500-DPI coordinates by eight.
        assert_eq!(bytes, [0, 0, 6, 32, 0, 1, 100, 208]);
    }
    #[test]
    fn compression_escapes_magic_and_splits_long_runs() {
        assert_eq!(compress(&[1, 1, 1, 0xc0, 2]), [0xc3, 1, 0xc1, 0xc0, 2]);
        assert_eq!(compress(&[0xff; 64]), [0xff, 0xff, 0xc1, 0xff]);
    }

    #[test]
    fn encodes_profile_speed_directly_as_tenths_of_percent() {
        let p = Project {
            speed_percent: 9.0,
            ..sample()
        };
        let prepared = prepare(&p).unwrap();
        let bytes = &prepared.jobs[0].bytes;
        assert!(bytes.windows(4).any(|b| b == [0x1b, 0x53, 0, 90]));
        let mut settings_bytes = Vec::new();
        settings(&mut settings_bytes, 20.0, 100.0);
        assert_eq!(&settings_bytes[..4], [0x1b, 0x53, 3, 232]);
    }
    #[test]
    fn rejects_zero_power_and_wrong_machine_dimensions() {
        let mut p = sample();
        p.power_percent = 0.0;
        assert!(prepare(&p).is_err());
        p.power_percent = 20.0;
        p.bed_height_mm = 400.0;
        assert!(prepare(&p).is_err());
    }
    #[test]
    fn raster_job_contains_compressed_scanlines() {
        let p = Project { operation: Operation::Engrave, width_mm: 10.0, height_mm: 6.0,
            svg: r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60"><rect x="20" y="20" width="60" height="20" fill="black"/></svg>"#.into(), ..sample() };
        let prepared = prepare(&p).unwrap();
        assert!(prepared.jobs[0].bytes.windows(2).any(|b| b == [0x1b, 0x30]));
        assert!(prepared.description.contains("Gravurzeilen"));
    }
    #[test]
    fn tcp_transfer_delivers_exact_bytes() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let receiver = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            bytes
        });
        transmit("127.0.0.1", port, b"test job").unwrap();
        assert_eq!(receiver.join().unwrap(), b"test job");
    }
}
