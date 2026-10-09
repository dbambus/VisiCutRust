//! AutoCAD Color Index (ACI) → RGB.

/// RGB colour of an ACI index (1–255). Index 7 ("white/black") is the
/// foreground colour and becomes black on VisiCut's white canvas.
pub fn aci(index: i64) -> u32 {
    const BASE: [u32; 10] = [
        0x000000, 0xFF0000, 0xFFFF00, 0x00FF00, 0x00FFFF, 0x0000FF, 0xFF00FF, 0x000000, 0x414141,
        0x808080,
    ];
    const GREYS: [u32; 6] = [0x333333, 0x505050, 0x696969, 0x828282, 0xBEBEBE, 0xFFFFFF];
    match index {
        0..=9 => BASE[index as usize],
        10..=249 => {
            // 24 hues in 15° steps, each with 5 brightness levels in a full and a
            // half-saturated variant.
            let hue = ((index / 10 - 1) * 15) as f64;
            let shade = index % 10;
            let value = [1.0, 0.8, 0.6, 0.5, 0.3][(shade / 2) as usize];
            let ramp = |offset: f64| -> f64 {
                let h = (hue - offset).rem_euclid(360.0);
                match h {
                    h if h <= 60.0 => 1.0,
                    h if h < 120.0 => (120.0 - h) / 60.0,
                    h if h <= 240.0 => 0.0,
                    h if h < 300.0 => (h - 240.0) / 60.0,
                    _ => 1.0,
                }
            };
            // Red peaks at 0°, green at 120°, blue at 240°.
            let channels = [ramp(0.0), ramp(120.0), ramp(240.0)].map(|c| {
                let full = 255.0 * c;
                let level = if shade % 2 == 1 {
                    full + (255.0 - full) / 2.0
                } else {
                    full
                };
                (level * value + 1e-6).floor() as u32
            });
            channels[0] << 16 | channels[1] << 8 | channels[2]
        }
        250..=255 => GREYS[(index - 250) as usize],
        _ => 0,
    }
}

pub fn hex(rgb: u32) -> String {
    format!("#{:06x}", rgb & 0xFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_autocad_palette() {
        assert_eq!(aci(1), 0xFF0000);
        assert_eq!(aci(7), 0x000000);
        assert_eq!(aci(10), 0xFF0000);
        assert_eq!(aci(11), 0xFF7F7F);
        assert_eq!(aci(13), 0xCC6666);
        assert_eq!(aci(20), 0xFF3F00);
        assert_eq!(aci(21), 0xFF9F7F);
        assert_eq!(aci(25), 0x995F4C);
        assert_eq!(aci(29), 0x4C2F26);
        assert_eq!(aci(30), 0xFF7F00);
        assert_eq!(aci(40), 0xFFBF00);
        assert_eq!(aci(90), 0x00FF00);
        assert_eq!(aci(150), 0x007FFF);
        assert_eq!(aci(170), 0x0000FF);
        assert_eq!(aci(210), 0xFF00FF);
        assert_eq!(aci(220), 0xFF00BF);
        assert_eq!(aci(250), 0x333333);
    }
}
