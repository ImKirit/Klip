//! Thin drawing layer over a Direct2D render target.
//!
//! The same view code measures and draws: a `Gfx` without a target lays out text and
//! returns sizes but draws nothing. The live windows (device context on a swap chain)
//! and the PNG snapshot (WIC bitmap target) both go through `ID2D1RenderTarget`.

use crate::fonts::{Fonts, Role, Text};
use crate::theme::Rgb;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D_RECT_F,
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

    /// HUD lettering: capitals, spaced by `tracking` as a share of the font size.
    pub fn caps(&self, text: &str, role: Role, weight: i32, size: f32, tracking: f32) -> Text {
        self.fonts.text(&text.to_uppercase(), role, weight, size, None, size * tracking)
    }

    pub fn caps_in(&self, text: &str, role: Role, weight: i32, size: f32, tracking: f32, max_w: f32) -> Text {
        self.fonts.text(&text.to_uppercase(), role, weight, size, Some(max_w.max(1.0)), size * tracking)
    }

    /// The mark of the design: a flat rectangle whose top right corner is cut off by
    /// `cut` pixels. No radius anywhere.
    pub fn cut(&self, x: f32, y: f32, w: f32, h: f32, cut: f32, c: Rgb, a: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let Some((rt, brush)) = &self.target else { return };
        let cut = cut.min(w).min(h).max(0.0);
        let at = |x: f32, y: f32| Vector2 { X: x, Y: y };
        unsafe {
            brush.SetColor(&color(c, a));
            let built = (|| {
                let geometry = rt.GetFactory()?.CreatePathGeometry()?;
                let sink = geometry.Open()?;
                sink.BeginFigure(at(x, y), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLines(&[at(x + w - cut, y), at(x + w, y + cut), at(x + w, y + h), at(x, y + h)]);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;
                Ok::<_, windows::core::Error>(geometry)
            })();
            match built {
                Ok(geometry) => rt.FillGeometry(&geometry, brush, None),
                // Without the geometry the shape is still there, only uncut.
                Err(_) => rt.FillRectangle(&D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, brush),
            }
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
