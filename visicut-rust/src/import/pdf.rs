use super::Imported;
use std::path::Path;

pub fn read_pdf(_path: &Path) -> Result<Imported, String> {
    Err("PDF-Import ist noch nicht verfügbar".into())
}

pub fn read_postscript(_path: &Path) -> Result<Imported, String> {
    Err("EPS/PS-Import ist noch nicht verfügbar".into())
}
