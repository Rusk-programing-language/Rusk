//! The Rusk logo.
//!
//! The source of truth is `assets/rusk-logo.svg`. Terminals cannot draw SVG,
//! so Unix shells use the small ASCII wordmark from [`crate::tui::BANNER`].
//! Windows packages ship the SVG beside `rusk.exe`, and `rusk logo` prints
//! that file instead, so the shipped logo is the same one in the README.

/// The bundled SVG, compiled into the tool so it works from any directory.
pub const SVG: &str = include_str!("../assets/rusk-logo.svg");

/// The logo text for this platform.
pub fn text() -> String {
    if cfg!(windows) {
        SVG.to_string()
    } else {
        crate::tui::BANNER.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_svg_logo_is_bundled() {
        assert!(SVG.starts_with("<svg"));
        assert!(SVG.contains("</svg>"));
        assert!(!text().is_empty());
    }
}
