use super::Imported;
use std::path::Path;

pub fn read_plf(_path: &Path) -> Result<Imported, String> {
    Err("PLF-Import ist noch nicht verfügbar".into())
}

pub fn read_parametric_svg(_path: &Path) -> Result<Imported, String> {
    Err("Parametrische SVG werden noch nicht unterstützt".into())
}

pub fn read_laser_script(_path: &Path) -> Result<Imported, String> {
    Err("LaserScript-Import ist noch nicht verfügbar".into())
}
