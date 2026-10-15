//! Thin drawing layer over a Direct2D render target.
//!
//! The same view code measures and draws: a `Gfx` without a target lays out text and
//! returns sizes but draws nothing. The live windows (device context on a swap chain)
//! and the PNG snapshot (WIC bitmap target) both go through `ID2D1RenderTarget`.

use crate::fonts::{Fonts, Role, Text};
use crate::theme::Rgb;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_COLOR_F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_LAYER_OPTIONS_NONE, D2D1_LAYER_PARAMETERS, D2D1_ROUNDED_RECT,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows_numerics::{Matrix3x2, Vector2};

pub(crate) fn color(c: Rgb, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.0 as f32 / 255.0, g: c.1 as f32 / 255.0, b: c.2 as f32 / 255.0, a }
}

pub(crate) struct Gfx<'a> {
    pub fonts: &'a Fonts,
    target: Option<(&'a ID2D1RenderTarget, ID2D1SolidColorBrush)>,
}

impl<'a> Gfx<'a> {
    /// Measures only.
    pub fn dry(fonts: &'a Fonts) -> Self {
        Gfx { fonts, target: None }
    }

    pub fn on(fonts: &'a Fonts, rt: &'a ID2D1RenderTarget) -> windows::core::Result<Self> {
        unsafe {
            let brush = rt.CreateSolidColorBrush(&color(Rgb(0, 0, 0), 1.0), None)?;
            // Sub pixel text needs an opaque background, the overlay has none.
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            Ok(Gfx { fonts, target: Some((rt, brush)) })
        }
    }

    pub fn drawing(&self) -> bool {
        self.target.is_some()
    }

    /// A copy that only measures, for a view that needs its own size before it draws.
    pub fn measuring(&self) -> Gfx<'a> {
        Gfx::dry(self.fonts)
    }

    pub fn text(&self, text: &str, role: Role, weight: i32, size: f32) -> Text {
        self.fonts.text(text, role, weight, size, None, 0.0)
    }

    pub fn text_in(&self, text: &str, role: Role, weight: i32, size: f32, max_w: f32) -> Text {
        self.fonts.text(text, role, weight, size, Some(max_w), 0.0)
    }

    /// Lettering of headings and buttons. Once spaced capitals, now plain text a little
    /// larger, so labels read at a glance. `tracking` is kept for the call sites.
    pub fn caps(&self, text: &str, role: Role, weight: i32, size: f32, _tracking: f32) -> Text {
        self.fonts.text(text, role, weight.max(600), size * 1.08, None, 0.0)
    }

    pub fn caps_in(&self, text: &str, role: Role, weight: i32, size: f32, _tracking: f32, max_w: f32) -> Text {
        self.fonts.text(text, role, weight.max(600), size * 1.08, Some(max_w.max(1.0)), 0.0)
    }

    /// A flat rectangle with rounded corners. `cut` is the size of the shape's corner as
    /// the first designs drew it cut off; it now sets the radius, so every call keeps its
    /// proportions.
    pub fn cut(&self, x: f32, y: f32, w: f32, h: f32, cut: f32, c: Rgb, a: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let Some((rt, brush)) = &self.target else { return };
        let radius = (cut * 0.75).min(w / 2.0).min(h / 2.0).max(0.0);
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        unsafe {
            brush.SetColor(&color(c, a));
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius }, brush);
        }
    }

    /// Segment meter: squares of `size` with `gap` between them, the first `on` in
    /// `lit`, the rest in `dark`.
    #[allow(clippy::too_many_arguments)]
    pub fn meter(&self, x: f32, y: f32, count: usize, on: usize, size: f32, gap: f32, lit: Rgb, dark: Rgb) {
        for i in 0..count {
            let c = if i < on { lit } else { dark };
            self.rect(x + i as f32 * (size + gap), y, size, size, 0.0, c, 1.0);
        }
    }

    pub fn put(&self, text: &Text, x: f32, y: f32, c: Rgb, a: f32) {
        if let (Some((rt, brush)), Some(layout)) = (&self.target, &text.layout) {
            unsafe {
                brush.SetColor(&color(c, a));
                rt.DrawTextLayout(Vector2 { X: x.round(), Y: y.round() }, layout, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
    }

    /// Text centered in a box.
    pub fn put_centered(&self, text: &Text, x: f32, y: f32, w: f32, h: f32, c: Rgb, a: f32) {
        self.put(text, x + (w - text.w) / 2.0, y + (h - text.h) / 2.0, c, a);
    }

    pub fn rect(&self, x: f32, y: f32, w: f32, h: f32, radius: f32, c: Rgb, a: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        if let Some((rt, brush)) = &self.target {
            let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
            let rr = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
                radiusX: r,
                radiusY: r,
            };
            unsafe {
                brush.SetColor(&color(c, a));
                rt.FillRoundedRectangle(&rr, brush);
            }
        }
    }

    /// A filled shape with a one pixel border, drawn as two fills so the edge stays
    /// crisp at any position.
    pub fn frame(&self, x: f32, y: f32, w: f32, h: f32, radius: f32, fill: Rgb, border: Rgb) {
        self.rect(x, y, w, h, radius, border, 1.0);
        self.rect(x + 1.0, y + 1.0, w - 2.0, h - 2.0, (radius - 1.0).max(0.0), fill, 1.0);
    }

    pub fn circle(&self, cx: f32, cy: f32, r: f32, c: Rgb, a: f32) {
        if let Some((rt, brush)) = &self.target {
            unsafe {
                brush.SetColor(&color(c, a));
                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r }, brush);
            }
        }
    }

    pub fn ring(&self, cx: f32, cy: f32, r: f32, width: f32, c: Rgb, a: f32) {
        if let Some((rt, brush)) = &self.target {
            unsafe {
                brush.SetColor(&color(c, a));
                rt.DrawEllipse(
                    &D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r },
                    brush,
                    width,
                    None::<&ID2D1StrokeStyle>,
                );
            }
        }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Rgb, a: f32) {
        if let Some((rt, brush)) = &self.target {
            unsafe {
                brush.SetColor(&color(c, a));
                rt.DrawLine(Vector2 { X: x0, Y: y0 }, Vector2 { X: x1, Y: y1 }, brush, width, None::<&ID2D1StrokeStyle>);
            }
        }
    }
}

/// Runs `draw` moved to `(x, y)` and, below full opacity, inside an opacity layer.
pub(crate) fn with_offset_and_opacity(rt: &ID2D1RenderTarget, x: f32, y: f32, opacity: f32, draw: impl FnOnce()) {
    unsafe {
        rt.SetTransform(&Matrix3x2::translation(x, y));
        let layered = opacity < 0.999;
        if layered {
            let params = D2D1_LAYER_PARAMETERS {
                contentBounds: D2D_RECT_F { left: -1.0e6, top: -1.0e6, right: 1.0e6, bottom: 1.0e6 },
                geometricMask: std::mem::ManuallyDrop::new(None),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: opacity.clamp(0.0, 1.0),
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            rt.PushLayer(&params, None);
        }
        draw();
        if layered {
            rt.PopLayer();
        }
        rt.SetTransform(&Matrix3x2::identity());
    }
}
