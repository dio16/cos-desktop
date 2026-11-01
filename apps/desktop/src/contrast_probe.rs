//! Read-only Windows High Contrast probe (MP-AX1 / AX04 limited slice).
//!
//! Windows reports the current scheme in the per-user registry: the
//! `HighContrast\Flags` value carries the on/off bit and `Control Panel\Colors`
//! carries the system colours a high-contrast theme installs. Both are read
//! through the operating system's `reg query` (read-only). This crate forbids its
//! own `unsafe`, so no Win32 signature is declared here, and the probe never
//! changes a setting.
//!
//! Failure is `None`, never a guessed scheme: the shell then keeps the approved
//! normal palette rather than inventing high-contrast colours.

use desktop_gpui::contrast::{ContrastScheme, SystemColors};

/// One probe reading: the scheme Windows reports and the system colours that
/// belong to it. `None` when either cannot be read.
pub fn read() -> Option<(ContrastScheme, SystemColors)> {
    #[cfg(windows)]
    let reading = windows_read();
    #[cfg(not(windows))]
    let reading: Option<(ContrastScheme, SystemColors)> = None;
    // Bounded, value-free trace for native evidence: the scheme only, never a colour
    // or a credential. Off unless the operator sets the variable.
    if std::env::var_os("COS_CONTRAST_TRACE").is_some() {
        match &reading {
            Some((scheme, _)) => eprintln!("contrast probe: {scheme:?}"),
            None => eprintln!("contrast probe: unavailable"),
        }
    }
    reading
}

#[cfg(windows)]
fn windows_read() -> Option<(ContrastScheme, SystemColors)> {
    let flags_text = query(&[
        "HKCU\\Control Panel\\Accessibility\\HighContrast",
        "/v",
        "Flags",
    ])?;
    let enabled = parse_flags(&flags_text)?;
    let colors_text = query(&["HKCU\\Control Panel\\Colors"])?;
    let colors = parse_colors(&colors_text)?;
    Some((
        if enabled {
            ContrastScheme::HighContrast
        } else {
            ContrastScheme::Normal
        },
        colors,
    ))
}

#[cfg(windows)]
fn query(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("reg.exe")
        .arg("query")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// `Flags` is a `REG_SZ` hexadecimal word; bit 0 is the on/off flag.
fn parse_flags(text: &str) -> Option<bool> {
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("Flags") {
            continue;
        }
        // `reg query` prints `Flags    REG_SZ    126`; the value is the last token.
        let value = fields.last()?.trim();
        let hex = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .unwrap_or(value);
        let flags = u32::from_str_radix(hex, 16).ok()?;
        return Some(flags & 1 != 0);
    }
    None
}

/// `Control Panel\Colors` values are `"R G B"` decimal strings.
fn parse_colors(text: &str) -> Option<SystemColors> {
    let mut window = None;
    let mut window_text = None;
    let mut highlight = None;
    let mut highlight_text = None;
    let mut highlight_alias = None;
    let mut highlight_text_alias = None;
    let mut gray_text = None;
    let mut button_face = None;
    let mut button_text = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else {
            continue;
        };
        // Windows stores the selection colours under the historical spellings
        // `Hilight`/`HilightText`. The longer spellings are accepted as a legacy
        // alias only; the canonical names win when both are present.
        let slot = match name {
            "Window" => &mut window,
            "WindowText" => &mut window_text,
            "Hilight" => &mut highlight,
            "HilightText" => &mut highlight_text,
            "Highlight" => &mut highlight_alias,
            "HighlightText" => &mut highlight_text_alias,
            "GrayText" => &mut gray_text,
            "ButtonFace" => &mut button_face,
            "ButtonText" => &mut button_text,
            _ => continue,
        };
        if let Some(rgb) = parse_rgb(&fields.collect::<Vec<_>>()) {
            *slot = Some(rgb);
        }
    }
    Some(SystemColors {
        window: window?,
        window_text: window_text?,
        // Canonical `Hilight`/`HilightText` win; the legacy spellings are the fallback.
        highlight: highlight.or(highlight_alias)?,
        highlight_text: highlight_text.or(highlight_text_alias)?,
        gray_text: gray_text?,
        button_face: button_face?,
        button_text: button_text?,
    })
}

/// The last three decimal tokens of a `reg query` value line.
fn parse_rgb(tokens: &[&str]) -> Option<u32> {
    if tokens.len() < 3 {
        return None;
    }
    let tail = &tokens[tokens.len() - 3..];
    let red: u32 = tail[0].parse().ok()?;
    let green: u32 = tail[1].parse().ok()?;
    let blue: u32 = tail[2].parse().ok()?;
    if red > 255 || green > 255 || blue > 255 {
        return None;
    }
    Some((red << 16) | (green << 8) | blue)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAGS_OFF: &str = "HKEY_CURRENT_USER\\Control Panel\\Accessibility\\HighContrast\r\n    Flags    REG_SZ    126\r\n";
    const FLAGS_ON: &str = "HKEY_CURRENT_USER\\Control Panel\\Accessibility\\HighContrast\r\n    Flags    REG_SZ    127\r\n";

    const COLORS: &str = "HKEY_CURRENT_USER\\Control Panel\\Colors\r\nButtonFace    REG_SZ    240 240 240\r\nButtonHilight    REG_SZ    255 255 255\r\nButtonText    REG_SZ    0 0 0\r\nGrayText    REG_SZ    109 109 109\r\nHilight    REG_SZ    0 120 215\r\nHilightText    REG_SZ    255 255 255\r\nMenuHilight    REG_SZ    0 120 215\r\nWindow    REG_SZ    255 255 255\r\nWindowFrame    REG_SZ    100 100 100\r\nWindowText    REG_SZ    0 0 0\r\n";

    #[test]
    fn flags_bit_reads_the_scheme() {
        assert_eq!(parse_flags(FLAGS_OFF), Some(false));
        assert_eq!(parse_flags(FLAGS_ON), Some(true));
        assert_eq!(parse_flags("no flags here"), None);
    }

    #[test]
    fn the_system_colors_are_read_verbatim() {
        let colors = parse_colors(COLORS).expect("a complete Colors key");
        assert_eq!(colors.window, 0xffffff);
        assert_eq!(colors.window_text, 0x000000);
        assert_eq!(colors.highlight, 0x0078d7);
        assert_eq!(colors.highlight_text, 0xffffff);
        assert_eq!(colors.gray_text, 0x6d6d6d);
        assert_eq!(colors.button_face, 0xf0f0f0);
        assert_eq!(colors.button_text, 0x000000);
    }

    #[test]
    fn the_legacy_selection_spellings_are_accepted() {
        let legacy = "\r\nWindow    REG_SZ    255 255 255\r\nWindowText    REG_SZ    0 0 0\r\nHighlight    REG_SZ    10 20 30\r\nHighlightText    REG_SZ    40 50 60\r\nGrayText    REG_SZ    70 80 90\r\nButtonFace    REG_SZ    1 2 3\r\nButtonText    REG_SZ    4 5 6\r\n";
        let colors = parse_colors(legacy).expect("the legacy spellings parse");
        assert_eq!(colors.highlight, 0x0a141e);
        assert_eq!(colors.highlight_text, 0x28323c);
        // Canonical names win when both spellings are present.
        let both = "\r\nWindow    REG_SZ    0 0 0\r\nWindowText    REG_SZ    0 0 0\r\nHighlight    REG_SZ    1 1 1\r\nHighlightText    REG_SZ    1 1 1\r\nHilight    REG_SZ    2 2 2\r\nHilightText    REG_SZ    3 3 3\r\nGrayText    REG_SZ    0 0 0\r\nButtonFace    REG_SZ    0 0 0\r\nButtonText    REG_SZ    0 0 0\r\n";
        let colors = parse_colors(both).expect("both spellings parse");
        assert_eq!(colors.highlight, 0x020202);
        assert_eq!(colors.highlight_text, 0x030303);
    }

    #[test]
    fn an_incomplete_colors_key_is_refused() {
        let partial =
            "    Window    REG_SZ    0 0 0\r\n    WindowText    REG_SZ    255 255 255\r\n";
        assert_eq!(parse_colors(partial), None);
        assert_eq!(parse_rgb(&["REG_SZ", "1", "2"]), None);
        assert_eq!(parse_rgb(&["REG_SZ", "300", "0", "0"]), None);
    }
}
