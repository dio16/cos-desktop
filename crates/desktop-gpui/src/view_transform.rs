//! Pure image-view geometry in logical desktop-independent DIP coordinates.
//!
//! Source coordinates and tile coordinates are always in the original, un-oriented image.

use media::Orientation;

/// Axis-aligned rectangle in the coordinate space named by the calling API.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Two-dimensional point in source pixels, DIP, or device pixels as documented by the API.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Positive dimensions. For `ViewTransform::new`, source dimensions are source pixels and
/// viewport dimensions are logical DIP.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// Error for invalid geometry at the public API boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformError {
    InvalidSource,
    InvalidViewport,
    InvalidDpi,
    InvalidArgument,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleMode {
    Fit,
    Physical,
}

/// A visible original-orientation source tile and its clipped pixel rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileCoord {
    pub x: u32,
    pub y: u32,
    pub source_rect: Rect,
}

const TILE_SIZE: f64 = 512.0;
const MAX_PENDING_TILES: usize = 16;
const MIN_SCALE: f64 = 1.0e-6;
const MAX_SCALE: f64 = 1.0e6;

/// Maps original source pixels to a logical-DIP viewport without owning renderer state.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewTransform {
    source: Size,
    viewport: Size,
    dpi: f64,
    orientation: Orientation,
    mode: ScaleMode,
    /// DIP per source pixel, before orientation (orientation preserves scale).
    scale: f64,
    /// Center in oriented source-pixel coordinates.
    center: Point,
}

impl ViewTransform {
    pub fn new(source: Size, viewport: Size, dpi: f64) -> Result<Self, TransformError> {
        validate_size(source).map_err(|_| TransformError::InvalidSource)?;
        validate_size(viewport).map_err(|_| TransformError::InvalidViewport)?;
        validate_dpi(dpi)?;
        let mut result = Self {
            source,
            viewport,
            dpi,
            orientation: Orientation::Normal,
            mode: ScaleMode::Fit,
            scale: 1.0,
            center: Point {
                x: source.width / 2.0,
                y: source.height / 2.0,
            },
        };
        result.fit();
        Ok(result)
    }

    pub fn source_size(&self) -> Size {
        self.source
    }
    pub fn viewport(&self) -> Size {
        self.viewport
    }
    pub fn dpi(&self) -> f64 {
        self.dpi
    }
    pub fn orientation(&self) -> Orientation {
        self.orientation
    }
    pub fn scale_mode(&self) -> ScaleMode {
        self.mode
    }

    /// Sets the display orientation while preserving the normalized center anchor.
    pub fn set_orientation(&mut self, orientation: Orientation) {
        let normalized = self.normalized_center();
        self.orientation = orientation;
        self.center = self.oriented_size().to_point_scaled(normalized);
        if self.mode == ScaleMode::Fit {
            self.recompute_fit_scale();
        }
    }

    /// Updates viewport and DPI while retaining the normalized image-center anchor. Physical mode
    /// retains device-pixel scale; fit mode recomputes the largest fit scale.
    pub fn set_viewport(&mut self, viewport: Size, dpi: f64) -> Result<(), TransformError> {
        validate_size(viewport).map_err(|_| TransformError::InvalidViewport)?;
        validate_dpi(dpi)?;
        let normalized = self.normalized_center();
        let physical_scale = self.effective_physical_scale();
        self.viewport = viewport;
        self.dpi = dpi;
        self.center = self.oriented_size().to_point_scaled(normalized);
        match self.mode {
            ScaleMode::Fit => self.recompute_fit_scale(),
            ScaleMode::Physical => self.scale = clamp_scale(physical_scale / dpi),
        }
        Ok(())
    }

    /// Fits the entire oriented image in the viewport and centers it.
    pub fn fit(&mut self) {
        self.mode = ScaleMode::Fit;
        self.center = Point {
            x: self.oriented_size().width / 2.0,
            y: self.oriented_size().height / 2.0,
        };
        self.recompute_fit_scale();
    }

    /// Sets one source pixel to one device pixel, retaining the normalized center anchor.
    pub fn physical_one_to_one(&mut self) {
        let normalized = self.normalized_center();
        self.mode = ScaleMode::Physical;
        self.center = self.oriented_size().to_point_scaled(normalized);
        self.scale = clamp_scale(1.0 / self.dpi);
    }

    /// Zooms around a cursor in viewport-local DIP, keeping the source point beneath it fixed.
    pub fn zoom_at(&mut self, factor: f64, cursor_dip: Point) -> Result<(), TransformError> {
        if !factor.is_finite() || factor <= 0.0 || !finite_point(cursor_dip) {
            return Err(TransformError::InvalidArgument);
        }
        let anchor = self.dip_to_oriented(cursor_dip);
        self.scale = clamp_scale(self.scale * factor);
        self.mode = ScaleMode::Physical;
        let offset = Point {
            x: cursor_dip.x - self.viewport.width / 2.0,
            y: cursor_dip.y - self.viewport.height / 2.0,
        };
        self.center = Point {
            x: anchor.x - offset.x / self.scale,
            y: anchor.y - offset.y / self.scale,
        };
        Ok(())
    }

    /// Moves the image by a viewport-local DIP delta.
    pub fn pan(&mut self, delta_dip: Point) -> Result<(), TransformError> {
        if !finite_point(delta_dip) {
            return Err(TransformError::InvalidArgument);
        }
        self.center.x -= delta_dip.x / self.scale;
        self.center.y -= delta_dip.y / self.scale;
        self.mode = ScaleMode::Physical;
        Ok(())
    }

    /// Display bounds in viewport-local DIP. The ideal center remains unbounded for normalized
    /// anchor preservation; drawing clamps to image edges, or centers images smaller than viewport.
    pub fn image_bounds(&self) -> Rect {
        let size = self.oriented_size();
        let draw_center = self.display_center();
        Rect {
            x: self.viewport.width / 2.0 - draw_center.x * self.scale,
            y: self.viewport.height / 2.0 - draw_center.y * self.scale,
            width: size.width * self.scale,
            height: size.height * self.scale,
        }
    }

    fn display_center(&self) -> Point {
        let size = self.oriented_size();
        let axis_center = |ideal: f64, extent: f64, viewport: f64| {
            if extent * self.scale <= viewport {
                extent / 2.0
            } else {
                let half_viewport = viewport / (2.0 * self.scale);
                ideal.clamp(half_viewport, extent - half_viewport)
            }
        };
        Point {
            x: axis_center(self.center.x, size.width, self.viewport.width),
            y: axis_center(self.center.y, size.height, self.viewport.height),
        }
    }

    /// Maps a point in original source pixels to viewport-local DIP.
    pub fn source_to_dip(&self, source: Point) -> Point {
        let oriented = self.source_to_oriented(source);
        let bounds = self.image_bounds();
        Point {
            x: bounds.x + oriented.x * self.scale,
            y: bounds.y + oriented.y * self.scale,
        }
    }

    /// Maps viewport-local DIP to original source pixels (coordinates may lie outside the image).
    pub fn dip_to_source(&self, dip: Point) -> Point {
        self.oriented_to_source(self.dip_to_oriented(dip))
    }

    /// Maps original source pixels to device pixels, using the configured DPI scale.
    pub fn source_to_device(&self, source: Point) -> Point {
        let p = self.source_to_dip(source);
        Point {
            x: p.x * self.dpi,
            y: p.y * self.dpi,
        }
    }

    /// Maps device pixels to original source pixels.
    pub fn device_to_source(&self, device: Point) -> Point {
        self.dip_to_source(Point {
            x: device.x / self.dpi,
            y: device.y / self.dpi,
        })
    }

    /// Effective device pixels per source pixel.
    pub fn effective_physical_scale(&self) -> f64 {
        self.scale * self.dpi
    }

    /// Suggested mip level for downscaling. Level zero is the source; scale >= 1 uses level zero.
    pub fn mip_level(&self) -> u32 {
        let scale = self.effective_physical_scale();
        if scale >= 1.0 {
            0
        } else {
            (-scale.log2()).floor().max(0.0) as u32
        }
    }

    /// Visible 512px source tiles in row-major order, limited to 16 pending tiles. Rectangles
    /// are clipped to source bounds and remain in original source orientation.
    pub fn visible_tiles(&self, max_pending: usize) -> Vec<TileCoord> {
        let bounds = self.image_bounds();
        let left = 0.0_f64.max(bounds.x);
        let top = 0.0_f64.max(bounds.y);
        let right = self.viewport.width.min(bounds.x + bounds.width);
        let bottom = self.viewport.height.min(bounds.y + bounds.height);
        let limit = max_pending.min(MAX_PENDING_TILES);
        if limit == 0 || right <= left || bottom <= top {
            return Vec::new();
        }
        let corners = [
            self.dip_to_source(Point { x: left, y: top }),
            self.dip_to_source(Point { x: right, y: top }),
            self.dip_to_source(Point { x: left, y: bottom }),
            self.dip_to_source(Point {
                x: right,
                y: bottom,
            }),
        ];
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for p in corners {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
        min_x = min_x.clamp(0.0, self.source.width);
        min_y = min_y.clamp(0.0, self.source.height);
        max_x = max_x.clamp(0.0, self.source.width);
        max_y = max_y.clamp(0.0, self.source.height);
        let x0 = (min_x / TILE_SIZE).floor() as u32;
        let y0 = (min_y / TILE_SIZE).floor() as u32;
        let x1 = ((max_x / TILE_SIZE).ceil() as u32).saturating_sub(1);
        let y1 = ((max_y / TILE_SIZE).ceil() as u32).saturating_sub(1);
        let cols = (self.source.width / TILE_SIZE).ceil() as u32;
        let rows = (self.source.height / TILE_SIZE).ceil() as u32;
        let mut tiles = Vec::with_capacity(limit);
        for y in y0.min(rows)..=y1.min(rows.saturating_sub(1)) {
            for x in x0.min(cols)..=x1.min(cols.saturating_sub(1)) {
                let sx = f64::from(x) * TILE_SIZE;
                let sy = f64::from(y) * TILE_SIZE;
                tiles.push(TileCoord {
                    x,
                    y,
                    source_rect: Rect {
                        x: sx,
                        y: sy,
                        width: (self.source.width - sx).min(TILE_SIZE),
                        height: (self.source.height - sy).min(TILE_SIZE),
                    },
                });
                if tiles.len() == limit {
                    return tiles;
                }
            }
        }
        tiles
    }

    fn recompute_fit_scale(&mut self) {
        let size = self.oriented_size();
        self.scale =
            clamp_scale((self.viewport.width / size.width).min(self.viewport.height / size.height));
    }
    fn oriented_size(&self) -> Size {
        match self.orientation {
            Orientation::Transpose
            | Orientation::Rotate90
            | Orientation::Transverse
            | Orientation::Rotate270 => Size {
                width: self.source.height,
                height: self.source.width,
            },
            _ => self.source,
        }
    }
    fn normalized_center(&self) -> Point {
        let size = self.oriented_size();
        Point {
            x: self.center.x / size.width,
            y: self.center.y / size.height,
        }
    }
    fn dip_to_oriented(&self, dip: Point) -> Point {
        let b = self.image_bounds();
        Point {
            x: (dip.x - b.x) / self.scale,
            y: (dip.y - b.y) / self.scale,
        }
    }
    fn source_to_oriented(&self, p: Point) -> Point {
        let w = self.source.width;
        let h = self.source.height;
        match self.orientation {
            Orientation::Normal => Point { x: p.x, y: p.y },
            Orientation::MirrorHorizontal => Point { x: w - p.x, y: p.y },
            Orientation::Rotate180 => Point {
                x: w - p.x,
                y: h - p.y,
            },
            Orientation::MirrorVertical => Point { x: p.x, y: h - p.y },
            Orientation::Transpose => Point { x: p.y, y: p.x },
            Orientation::Rotate90 => Point { x: h - p.y, y: p.x },
            Orientation::Transverse => Point {
                x: h - p.y,
                y: w - p.x,
            },
            Orientation::Rotate270 => Point { x: p.y, y: w - p.x },
        }
    }
    fn oriented_to_source(&self, p: Point) -> Point {
        let w = self.source.width;
        let h = self.source.height;
        match self.orientation {
            Orientation::Normal => Point { x: p.x, y: p.y },
            Orientation::MirrorHorizontal => Point { x: w - p.x, y: p.y },
            Orientation::Rotate180 => Point {
                x: w - p.x,
                y: h - p.y,
            },
            Orientation::MirrorVertical => Point { x: p.x, y: h - p.y },
            Orientation::Transpose => Point { x: p.y, y: p.x },
            Orientation::Rotate90 => Point { x: p.y, y: h - p.x },
            Orientation::Transverse => Point {
                x: w - p.y,
                y: h - p.x,
            },
            Orientation::Rotate270 => Point { x: w - p.y, y: p.x },
        }
    }
}

trait SizePoint {
    fn to_point_scaled(self, p: Point) -> Point;
}
impl SizePoint for Size {
    fn to_point_scaled(self, p: Point) -> Point {
        Point {
            x: self.width * p.x,
            y: self.height * p.y,
        }
    }
}

fn validate_size(size: Size) -> Result<(), TransformError> {
    if size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0 {
        Ok(())
    } else {
        Err(TransformError::InvalidArgument)
    }
}
fn validate_dpi(dpi: f64) -> Result<(), TransformError> {
    if dpi.is_finite() && dpi > 0.0 {
        Ok(())
    } else {
        Err(TransformError::InvalidDpi)
    }
}
fn finite_point(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}
fn clamp_scale(scale: f64) -> f64 {
    scale.clamp(MIN_SCALE, MAX_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1.0e-8, "{a} != {b}");
    }
    fn view(dpi: f64) -> ViewTransform {
        ViewTransform::new(
            Size {
                width: 2048.0,
                height: 2048.0,
            },
            Size {
                width: 1200.0,
                height: 900.0,
            },
            dpi,
        )
        .unwrap()
    }

    #[test]
    fn physical_one_to_one_is_one_device_pixel_at_common_dpis() {
        for dpi in [1.0, 1.25, 1.5, 2.0] {
            let mut v = view(dpi);
            v.physical_one_to_one();
            close(v.effective_physical_scale(), 1.0);
            let p = v.source_to_device(Point {
                x: 1024.0,
                y: 1024.0,
            });
            close(p.x, 600.0 * dpi);
            close(p.y, 450.0 * dpi);
        }
    }

    #[test]
    fn zoom_keeps_cursor_anchor_fixed() {
        let mut v = view(1.5);
        let cursor = Point { x: 311.0, y: 270.0 };
        let before = v.dip_to_source(cursor);
        v.zoom_at(2.0, cursor).unwrap();
        let after = v.dip_to_source(cursor);
        close(before.x, after.x);
        close(before.y, after.y);
    }

    #[test]
    fn finite_extreme_zoom_is_clamped() {
        let mut v = view(1.0);
        v.zoom_at(f64::MAX, Point { x: 0.0, y: 0.0 }).unwrap();
        close(v.scale, MAX_SCALE);
        v.zoom_at(f64::MIN_POSITIVE, Point { x: 0.0, y: 0.0 })
            .unwrap();
        close(v.scale, MIN_SCALE);
    }

    #[test]
    fn resize_keeps_normalized_center_and_physical_scale() {
        let mut v = view(1.25);
        v.physical_one_to_one();
        v.pan(Point { x: 100.0, y: -50.0 }).unwrap();
        let center = v.normalized_center();
        v.set_viewport(
            Size {
                width: 800.0,
                height: 700.0,
            },
            2.0,
        )
        .unwrap();
        let resized = v.normalized_center();
        close(center.x, resized.x);
        close(center.y, resized.y);
        close(v.effective_physical_scale(), 1.0);
    }

    #[test]
    fn orientation_math_round_trips_all_exif_transforms() {
        for orientation in [
            Orientation::Normal,
            Orientation::MirrorHorizontal,
            Orientation::Rotate180,
            Orientation::MirrorVertical,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Transverse,
            Orientation::Rotate270,
        ] {
            let mut v = ViewTransform::new(
                Size {
                    width: 400.0,
                    height: 200.0,
                },
                Size {
                    width: 800.0,
                    height: 600.0,
                },
                1.0,
            )
            .unwrap();
            v.set_orientation(orientation);
            let b = v.image_bounds();
            let rotated = matches!(
                orientation,
                Orientation::Transpose
                    | Orientation::Rotate90
                    | Orientation::Transverse
                    | Orientation::Rotate270
            );
            close(b.width, if rotated { 300.0 } else { 800.0 });
            close(b.height, if rotated { 600.0 } else { 400.0 });
            let p = Point { x: 75.0, y: 123.0 };
            let roundtrip = v.dip_to_source(v.source_to_dip(p));
            close(roundtrip.x, p.x);
            close(roundtrip.y, p.y);
        }
    }

    #[test]
    fn pan_clamps_to_image_edges_and_centers_small_images() {
        let mut large = ViewTransform::new(
            Size {
                width: 2048.0,
                height: 2048.0,
            },
            Size {
                width: 512.0,
                height: 512.0,
            },
            1.0,
        )
        .unwrap();
        large.physical_one_to_one();
        let before_zero_pan = large.image_bounds();
        large.pan(Point { x: 0.0, y: 0.0 }).unwrap();
        assert_eq!(large.image_bounds(), before_zero_pan);
        large.pan(Point { x: 1.0e9, y: 1.0e9 }).unwrap();
        let left_edge = large.image_bounds();
        assert_eq!(left_edge.x, 0.0);
        assert_eq!(left_edge.y, 0.0);
        large
            .pan(Point {
                x: -2.0e9,
                y: -2.0e9,
            })
            .unwrap();
        let right_edge = large.image_bounds();
        assert_eq!(right_edge.x + right_edge.width, large.viewport.width);
        assert_eq!(right_edge.y + right_edge.height, large.viewport.height);

        let mut small = ViewTransform::new(
            Size {
                width: 100.0,
                height: 100.0,
            },
            Size {
                width: 1000.0,
                height: 800.0,
            },
            1.0,
        )
        .unwrap();
        small.physical_one_to_one();
        small
            .pan(Point {
                x: 1.0e9,
                y: -1.0e9,
            })
            .unwrap();
        let centered = small.image_bounds();
        assert_eq!(centered.x, 450.0);
        assert_eq!(centered.y, 350.0);
    }

    #[test]
    fn visible_tiles_exclude_off_view_tiles_and_cap_pending() {
        let mut v = ViewTransform::new(
            Size {
                width: 2048.0,
                height: 2048.0,
            },
            Size {
                width: 512.0,
                height: 512.0,
            },
            1.0,
        )
        .unwrap();
        v.physical_one_to_one();
        let tiles = v.visible_tiles(64);
        assert!(tiles.len() <= 16);
        assert!(tiles.iter().all(|t| t.x < 4 && t.y < 4));
        assert!(!tiles.iter().any(|t| t.x == 0 && t.y == 0));
        assert!(!tiles.iter().any(|t| t.x == 3 && t.y == 3));
        assert!(
            tiles
                .iter()
                .all(|t| t.source_rect.width <= 512.0 && t.source_rect.height <= 512.0)
        );
        assert!(
            tiles
                .iter()
                .all(|t| t.source_rect.x == f64::from(t.x) * 512.0)
        );
        assert!(
            tiles
                .iter()
                .all(|t| t.source_rect.y == f64::from(t.y) * 512.0)
        );
    }
}
