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
    let mut gray_text = None;
    let mut button_face = None;
    let mut button_text = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else {
            continue;
        };
        let slot = match name {
            "Window" => &mut window,
            "WindowText" => &mut window_text,
            "Highlight" => &mut highlight,
            "HighlightText" => &mut highlight_text,
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
        highlight: highlight?,
        highlight_text: highlight_text?,
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

    const COLORS: &str = "HKEY_CURRENT_USER\\Control Panel\\Colors\r\n    Window    REG_SZ    0 0 0\r\n    WindowText    REG_SZ    255 255 255\r\n    Highlight    REG_SZ    26 235 255\r\n    HighlightText    REG_SZ    0 0 0\r\n    GrayText    REG_SZ    140 140 140\r\n    ButtonFace    REG_SZ    32 32 32\r\n    ButtonText    REG_SZ    255 255 0\r\n";

    #[test]
    fn flags_bit_reads_the_scheme() {
        assert_eq!(parse_flags(FLAGS_OFF), Some(false));
        assert_eq!(parse_flags(FLAGS_ON), Some(true));
        assert_eq!(parse_flags("no flags here"), None);
    }

    #[test]
    fn the_system_colors_are_read_verbatim() {
        let colors = parse_colors(COLORS).expect("a complete Colors key");
        assert_eq!(colors.window, 0x000000);
        assert_eq!(colors.window_text, 0xffffff);
        assert_eq!(colors.highlight, 0x1aebff);
        assert_eq!(colors.highlight_text, 0x000000);
        assert_eq!(colors.gray_text, 0x8c8c8c);
        assert_eq!(colors.button_face, 0x202020);
        assert_eq!(colors.button_text, 0xffff00);
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
