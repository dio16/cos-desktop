/// Logical-pixel initial geometry. Persistence is deliberately not implemented:
/// canonical projections and credentials must never enter a preference store.
#[derive(Clone, Copy, Debug)]
pub struct WindowPreferences {
    pub width: f32,
    pub height: f32,
}

impl Default for WindowPreferences {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 800.0,
        }
    }
}
