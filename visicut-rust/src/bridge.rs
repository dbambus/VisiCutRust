//! Stateless C ABI for the native macOS shell. The caller owns requests and
//! must release each response with visicut_free. No Rust state crosses the ABI.
use crate::{ltt, project::Project, svg};
use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char};

fn get_project(request: &Value) -> Result<Project, String> {
    serde_json::from_value(request["project"].clone()).map_err(|e| e.to_string())
}

fn preview(source: &str) -> Result<Value, String> {
    let rendered = svg::render(source)?;
    let size = resvg::tiny_skia::IntSize::from_wh(
        rendered.image.size[0] as u32,
        rendered.image.size[1] as u32,
    )
    .ok_or("Ungültige Vorschaugröße")?;
    let pixels: Vec<u8> = rendered
        .image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_array())
        .collect();
    let pixmap =
        resvg::tiny_skia::Pixmap::from_vec(pixels, size).ok_or("Ungültige Vorschaupixel")?;
    let png = pixmap.encode_png().map_err(|e| e.to_string())?;
    Ok(json!({"width_mm": rendered.width_mm, "height_mm": rendered.height_mm, "png": png}))
}

fn read(path: &str) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > 25 * 1024 * 1024 {
        return Err("Datei ist größer als 25 MB".into());
    }
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

pub(crate) fn execute(request: &Value) -> Result<Value, String> {
    match request["action"].as_str().unwrap_or("") {
        "default" => Ok(json!({"project": Project::default()})),
        "materials" => serde_json::from_str(include_str!("../resources/materials.json"))
            .map_err(|e| e.to_string()),
        "demo" | "import" => {
            let mut project = get_project(request)?;
            project.svg = if request["action"] == "demo" {
                include_str!("../examples/demo.svg").into()
            } else {
                read(request["path"].as_str().ok_or("Dateipfad fehlt")?)?
            };
            project.steps.clear();
            let image = preview(&project.svg)?;
            project.width_mm = image["width_mm"].as_f64().unwrap() as f32;
            project.height_mm = image["height_mm"].as_f64().unwrap() as f32;
            project.name = if request["action"] == "demo" {
                "Beispiel".into()
            } else {
                std::path::Path::new(request["path"].as_str().unwrap())
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            };
            Ok(
                json!({"project": project, "preview": image, "objects": crate::selection::objects(&project.svg)?}),
            )
        }
        "load" => {
            let source = read(request["path"].as_str().ok_or("Dateipfad fehlt")?)?;
            let project: Project = serde_json::from_str(&source).map_err(|e| e.to_string())?;
            project.validate_document()?;
            let image = if project.svg.is_empty() {
                Value::Null
            } else {
                preview(&project.svg)?
            };
            Ok(
                json!({"project": project, "preview": image, "objects": crate::selection::objects(&project.svg)?}),
            )
        }
        "validate_document" => {
            get_project(request)?.validate_document()?;
            Ok(json!({}))
        }
        "prepare" => {
            let prepared = ltt::prepare(&get_project(request)?)?;
            Ok(
                json!({"jobs": prepared.jobs, "description": prepared.description,
                "estimated_seconds": prepared.estimated_seconds, "preview_png": prepared.preview_png,
                "steps": prepared.steps, "timeline": prepared.timeline}),
            )
        }
        "transmit" => {
            let project = get_project(request)?;
            let prepared = ltt::prepare(&project)?;
            let sent = ltt::transmit_jobs(&project.hostname, project.port, &prepared.jobs)?;
            Ok(json!({"description": prepared.description, "sent": sent}))
        }
        _ => Err("Unbekannte Aktion".into()),
    }
}

/// # Safety
/// `request` must be null or point to a valid NUL-terminated UTF-8 C string
/// for the entire duration of this call. Free the returned pointer exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn visicut_execute(request: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if request.is_null() {
            return Err("Leere Anfrage".to_string());
        }
        let request = unsafe { CStr::from_ptr(request) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_str(request).map_err(|e| e.to_string())?;
        execute(&value)
    });
    let response = match result {
        Ok(Ok(value)) => json!({"ok": true, "result": value}),
        Ok(Err(error)) => json!({"ok": false, "error": error}),
        Err(_) => json!({"ok": false, "error": "Rust-Verarbeitung unerwartet abgebrochen"}),
    };
    CString::new(response.to_string())
        .expect("JSON cannot contain literal NUL")
        .into_raw()
}

/// # Safety
/// `response` must be null or a live pointer returned by `visicut_execute`,
/// and must not have been freed previously.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn visicut_free(response: *mut c_char) {
    if !response.is_null() {
        drop(unsafe { CString::from_raw(response) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ffi_returns_owned_json_and_reports_errors() {
        let request = CString::new(r#"{"action":"default"}"#).unwrap();
        unsafe {
            let response = visicut_execute(request.as_ptr());
            let value: Value =
                serde_json::from_str(CStr::from_ptr(response).to_str().unwrap()).unwrap();
            assert_eq!(value["result"]["project"]["bed_width_mm"], 1000.0);
            visicut_free(response);
            let response = visicut_execute(std::ptr::null());
            let value: Value =
                serde_json::from_str(CStr::from_ptr(response).to_str().unwrap()).unwrap();
            assert_eq!(value["ok"], false);
            visicut_free(response);
        }
    }
    #[test]
    fn demo_bridge_produces_a_decodable_png() {
        let result = execute(&json!({"action": "demo", "project": Project::default()})).unwrap();
        let png: Vec<u8> = serde_json::from_value(result["preview"]["png"].clone()).unwrap();
        assert!(resvg::tiny_skia::Pixmap::decode_png(&png).is_ok());
        assert_eq!(result["project"]["width_mm"], 100.0);
    }
    #[test]
    fn new_artwork_resets_old_assignments_and_returns_object_list() {
        let mut p = Project::default();
        p.steps.push(crate::project::JobStep {
            operation: crate::project::Operation::Cut,
            objects: vec![999],
            power_percent: 20.0,
            speed_percent: 10.0,
            passes: 1,
        });
        let result = execute(&json!({"action": "demo", "project": p})).unwrap();
        assert_eq!(result["objects"].as_array().unwrap().len(), 3);
        assert!(result["project"]["steps"].as_array().unwrap().is_empty());
    }

    #[test]
    fn catalog_contains_fau_acrylic_3mm_cut_profile() {
        let catalog = execute(&json!({"action": "materials"})).unwrap();
        let acrylic = catalog["materials"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == "Acryl")
            .unwrap();
        let profile = acrylic["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["thickness_mm"] == 3.0 && p["operation"] == "Cut")
            .unwrap();
        assert_eq!(profile["power_percent"], 100.0);
        assert_eq!(profile["speed_percent"], 9.0);
    }
}
