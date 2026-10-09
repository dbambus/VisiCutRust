use super::Imported;
use std::path::Path;

pub fn read(_path: &Path) -> Result<Imported, String> {
    Err("DXF-Import ist noch nicht verfügbar".into())
}
