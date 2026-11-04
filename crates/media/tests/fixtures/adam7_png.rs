/// A valid Adam7-interlaced 8-bit RGB PNG, 19x11, built from the pattern
/// `(x*7 + y*3, x*5 + y*11, x + y)`. Generated once and embedded, because no encoder in this
/// workspace writes interlaced output: the fixture is the only way to show that a real Adam7
/// image still decodes through the product path.
pub const ADAM7_PNG_BASE64: &str = concat!(
    "iVBORw0KGgoAAAANSUhEUgAAABMAAAALCAIAAAFlsBH7AAACg0lEQVR42gXB11PScQAA8K8yfnsPlrk191bcCiouUFygICoo",
    "ioKKoOBCxYV7pmXZuMY17qq76qEe6qWX/qw+HwAAqMyC/WYaKAdh8xZ98EEJEnhpfy0Oyny47xEP8DxpViPc3I+Peej1dR6Q",
    "aHxOotxQgroM5IaNBeoKeUUH2jtCzgXYo30RZLpQfYgcOWJXX4i339RALsbzydLUHHlRGVxfjxrbcFsv6RmmQ5Psrp8HrF6e",
    "bIQLLGjtGN4xQw4u0pMb7OIBv30lXtwrQeIcmreMV2+TbSe05ZadeMUHPolbP5Rnf9T3/xIAJIsTKEmaSlacBjXkIyYtZtcR",
    "051UeIABhEaiyZBlF0LaKqSlCeszEU4rNe9kIl4OcMWylBqosAWp68Y6h4ihcWpqllkKcztRAahaoYc9SLkda3ITPfPU6Aoz",
    "u8OtnQqHjxUgyYHkT2E1C0T7GmXdY9znXPBOiL5RnH9WgYwgVhohdDGq+5JxPOO874SVr4rYL9XNXw2QYHEoHc8IEqVampQk",
    "y0yX52dDZQVwTSmir0Tb6zCzHre2EiNG0t1D+Sx00M6sjrFRNweQFAmdKVXkyhKL5BnlUF41XNqAVDejunasrQvv7iMsg6TD",
    "QU24aO8UE/CxKwvcVoiPrQmA0srEWvkDHZRugHM7kRIzWjWANdrw1lGia4IcmKaG5+jxIDOzzC5EuOVtfjMm7J+Ip5cKIJig",
    "hF44zYrkDKPFTqxyEm/wEgY/aVqi+ldp+ybj2mWnDzn/GR++FjaeiHvPFSevldfvVUAzjqR60OxZrCiAa8NE/TrZEqWM+3Tf",
    "MWO7YJ03nOcpP/9SCL0VIx8Vu1+Ux99VVz/Vd781/wGmkJzBCbmIVwAAAABJRU5ErkJggg==",
);

pub const ADAM7_WIDTH: u32 = 19;
pub const ADAM7_HEIGHT: u32 = 11;

/// The pixel the fixture was built with, so a decoded raster can be compared sample by sample.
pub fn adam7_expected_pixel(x: u32, y: u32) -> [u8; 3] {
    [
        ((x * 7 + y * 3) % 256) as u8,
        ((x * 5 + y * 11) % 256) as u8,
        ((x + y) % 256) as u8,
    ]
}
