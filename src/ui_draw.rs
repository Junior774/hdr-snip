#[derive(Debug, Clone, Copy)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

pub struct Canvas<'a> {
    buf: &'a mut [u8],
    width: i32,
    height: i32,
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut [u8], width: i32, height: i32) -> Self {
        Self { buf, width, height }
    }

    #[inline(always)]
    fn blend_px(&mut self, x: i32, y: i32, col: Rgba, coverage: f32) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height || coverage <= 0.002 {
            return;
        }
        let alpha = (col.a as f32 * (1.0 / 255.0)) * coverage.min(1.0);
        if alpha <= 0.002 {
            return;
        }
        let idx = ((y * self.width + x) * 4) as usize;
        if alpha >= 0.998 {
            self.buf[idx] = col.b;
            self.buf[idx + 1] = col.g;
            self.buf[idx + 2] = col.r;
            self.buf[idx + 3] = 255;
        } else {
            let inv = 1.0 - alpha;
            let db = self.buf[idx] as f32;
            let dg = self.buf[idx + 1] as f32;
            let dr = self.buf[idx + 2] as f32;
            self.buf[idx] = (col.b as f32 * alpha + db * inv + 0.5) as u8;
            self.buf[idx + 1] = (col.g as f32 * alpha + dg * inv + 0.5) as u8;
            self.buf[idx + 2] = (col.r as f32 * alpha + dr * inv + 0.5) as u8;
            self.buf[idx + 3] = 255;
        }
    }

    #[inline(always)]
    fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
        let r = r.min(hw).min(hh).max(0.0);
        let qx = (px - cx).abs() - hw + r;
        let qy = (py - cy).abs() - hh + r;
        let out = qx.max(0.0).hypot(qy.max(0.0));
        let inside = qx.max(qy).min(0.0);
        out + inside - r
    }

    pub fn draw_soft_shadow(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        offset_y: f32,
        blur: f32,
        max_alpha: f32,
    ) {
        if w <= 0.0 || h <= 0.0 || blur <= 0.5 {
            return;
        }
        let cx = x + w * 0.5;
        let cy = y + offset_y + h * 0.5;
        let hw = w * 0.5;
        let hh = h * 0.5;

        let x0 = ((x - blur).floor() as i32).max(0);
        let y0 = ((y + offset_y - blur).floor() as i32).max(0);
        let x1 = ((x + w + blur).ceil() as i32).min(self.width - 1);
        let y1 = ((y + offset_y + h + blur).ceil() as i32).min(self.height - 1);

        let inv_blur = 1.0 / blur;
        let black = Rgba::rgb(0, 0, 0);

        for py in y0..=y1 {
            let fy = py as f32 + 0.5;
            for px in x0..=x1 {
                let fx = px as f32 + 0.5;
                let sd = Self::sd_round_rect(fx, fy, cx, cy, hw, hh, r);
                if sd >= blur {
                    continue;
                }
                let t = (1.0 - sd.max(0.0) * inv_blur).clamp(0.0, 1.0);

                let falloff = t * t * (3.0 - 2.0 * t);
                let cov = max_alpha * falloff * falloff;
                self.blend_px(px, py, black, cov);
            }
        }
    }

    pub fn fill_round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, col: Rgba) {
        if w <= 0.0 || h <= 0.0 || col.a == 0 {
            return;
        }
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let hw = w * 0.5;
        let hh = h * 0.5;

        let x0 = ((x - 1.0).floor() as i32).max(0);
        let y0 = ((y - 1.0).floor() as i32).max(0);
        let x1 = ((x + w + 1.0).ceil() as i32).min(self.width - 1);
        let y1 = ((y + h + 1.0).ceil() as i32).min(self.height - 1);

        for py in y0..=y1 {
            let fy = py as f32 + 0.5;
            for px in x0..=x1 {
                let fx = px as f32 + 0.5;
                let sd = Self::sd_round_rect(fx, fy, cx, cy, hw, hh, r);
                let cov = (0.5 - sd).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, col, cov);
                }
            }
        }
    }

    pub fn stroke_round_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        thickness: f32,
        col: Rgba,
    ) {
        if w <= 0.0 || h <= 0.0 || thickness <= 0.0 || col.a == 0 {
            return;
        }
        let half_t = thickness * 0.5;
        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let hw = (w * 0.5 - half_t).max(0.0);
        let hh = (h * 0.5 - half_t).max(0.0);
        let ir = (r - half_t).max(0.0);

        let x0 = ((x - 1.0).floor() as i32).max(0);
        let y0 = ((y - 1.0).floor() as i32).max(0);
        let x1 = ((x + w + 1.0).ceil() as i32).min(self.width - 1);
        let y1 = ((y + h + 1.0).ceil() as i32).min(self.height - 1);

        for py in y0..=y1 {
            let fy = py as f32 + 0.5;
            for px in x0..=x1 {
                let fx = px as f32 + 0.5;
                let sd = Self::sd_round_rect(fx, fy, cx, cy, hw, hh, ir);
                let cov = (0.5 + half_t - sd.abs()).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, col, cov);
                }
            }
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, col: Rgba) {
        let x0 = ((cx - radius - 1.0).floor() as i32).max(0);
        let y0 = ((cy - radius - 1.0).floor() as i32).max(0);
        let x1 = ((cx + radius + 1.0).ceil() as i32).min(self.width - 1);
        let y1 = ((cy + radius + 1.0).ceil() as i32).min(self.height - 1);

        for py in y0..=y1 {
            let dy = py as f32 + 0.5 - cy;
            for px in x0..=x1 {
                let dx = px as f32 + 0.5 - cx;
                let sd = dx.hypot(dy) - radius;
                let cov = (0.5 - sd).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, col, cov);
                }
            }
        }
    }

    pub fn fill_rect_i32(&mut self, x: i32, y: i32, w: i32, h: i32, col: Rgba) {
        if w <= 0 || h <= 0 || col.a == 0 {
            return;
        }
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w - 1).min(self.width - 1);
        let y1 = (y + h - 1).min(self.height - 1);
        for py in y0..=y1 {
            for px in x0..=x1 {
                self.blend_px(px, py, col, 1.0);
            }
        }
    }

    pub fn draw_line_aa(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, thickness: f32, col: Rgba) {
        let half_t = thickness * 0.5;
        let pad = half_t + 1.5;
        let min_x = (x0.min(x1) - pad).floor() as i32;
        let min_y = (y0.min(y1) - pad).floor() as i32;
        let max_x = (x0.max(x1) + pad).ceil() as i32;
        let max_y = (y0.max(y1) + pad).ceil() as i32;

        let bx0 = min_x.max(0);
        let by0 = min_y.max(0);
        let bx1 = max_x.min(self.width - 1);
        let by1 = max_y.min(self.height - 1);

        let vx = x1 - x0;
        let vy = y1 - y0;
        let len_sq = vx * vx + vy * vy;

        for py in by0..=by1 {
            let fy = py as f32 + 0.5;
            for px in bx0..=bx1 {
                let fx = px as f32 + 0.5;
                let d = if len_sq < 1e-6 {
                    (fx - x0).hypot(fy - y0)
                } else {
                    let t = (((fx - x0) * vx + (fy - y0) * vy) / len_sq).clamp(0.0, 1.0);
                    let proj_x = x0 + t * vx;
                    let proj_y = y0 + t * vy;
                    (fx - proj_x).hypot(fy - proj_y)
                };
                let cov = (0.5 + half_t - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_px(px, py, col, cov);
                }
            }
        }
    }

    pub fn draw_icon_copy(&mut self, cx: f32, cy: f32, s: f32, col: Rgba) {
        let t = (1.55 * s).max(1.2);

        self.stroke_round_rect(
            cx - 2.0 * s,
            cy - 2.0 * s,
            8.0 * s,
            8.0 * s,
            2.0 * s,
            t,
            col,
        );

        let bx = cx - 5.2 * s;
        let by = cy - 5.2 * s;
        self.draw_line_aa(bx, by + 6.4 * s, bx, by + 1.4 * s, t, col);
        self.draw_line_aa(bx, by + 1.4 * s, bx + 1.4 * s, by, t, col);
        self.draw_line_aa(bx + 1.4 * s, by, bx + 6.4 * s, by, t, col);
    }

    pub fn draw_icon_save(&mut self, cx: f32, cy: f32, s: f32, col: Rgba) {
        let t = (1.55 * s).max(1.2);

        self.draw_line_aa(cx, cy - 5.5 * s, cx, cy + 1.6 * s, t, col);
        self.draw_line_aa(cx - 3.0 * s, cy - 1.3 * s, cx, cy + 1.8 * s, t, col);
        self.draw_line_aa(cx + 3.0 * s, cy - 1.3 * s, cx, cy + 1.8 * s, t, col);

        self.draw_line_aa(
            cx - 5.4 * s,
            cy + 2.4 * s,
            cx - 5.4 * s,
            cy + 5.2 * s,
            t,
            col,
        );
        self.draw_line_aa(
            cx - 5.4 * s,
            cy + 5.2 * s,
            cx + 5.4 * s,
            cy + 5.2 * s,
            t,
            col,
        );
        self.draw_line_aa(
            cx + 5.4 * s,
            cy + 5.2 * s,
            cx + 5.4 * s,
            cy + 2.4 * s,
            t,
            col,
        );
    }

    pub fn draw_icon_fullscreen(&mut self, cx: f32, cy: f32, s: f32, col: Rgba) {
        let t = (1.55 * s).max(1.2);
        let d = 5.2 * s;
        let leg = 3.1 * s;

        self.draw_line_aa(cx - d, cy - d + leg, cx - d, cy - d, t, col);
        self.draw_line_aa(cx - d, cy - d, cx - d + leg, cy - d, t, col);

        self.draw_line_aa(cx + d - leg, cy - d, cx + d, cy - d, t, col);
        self.draw_line_aa(cx + d, cy - d, cx + d, cy - d + leg, t, col);

        self.draw_line_aa(cx - d, cy + d - leg, cx - d, cy + d, t, col);
        self.draw_line_aa(cx - d, cy + d, cx - d + leg, cy + d, t, col);

        self.draw_line_aa(cx + d - leg, cy + d, cx + d, cy + d, t, col);
        self.draw_line_aa(cx + d, cy + d, cx + d, cy + d - leg, t, col);
    }

    pub fn draw_icon_close(&mut self, cx: f32, cy: f32, s: f32, col: Rgba) {
        let t = (1.75 * s).max(1.3);
        let d = 4.0 * s;
        self.draw_line_aa(cx - d, cy - d, cx + d, cy + d, t, col);
        self.draw_line_aa(cx + d, cy - d, cx - d, cy + d, t, col);
    }

    pub fn draw_icon_return(&mut self, cx: f32, cy: f32, s: f32, col: Rgba) {
        let t = (1.35 * s).max(1.1);
        let rx = cx + 3.2 * s;
        let lx = cx - 3.2 * s;
        let ty = cy - 2.6 * s;
        let by = cy + 0.7 * s;
        self.draw_line_aa(rx, ty, rx, by, t, col);
        self.draw_line_aa(rx, by, lx, by, t, col);
        self.draw_line_aa(lx, by, lx + 2.2 * s, by - 2.1 * s, t, col);
        self.draw_line_aa(lx, by, lx + 2.2 * s, by + 2.1 * s, t, col);
    }

    pub fn draw_corner_bracket(
        &mut self,
        corner_x: i32,
        corner_y: i32,
        dx: i32,
        dy: i32,
        leg: i32,
        thick: i32,
    ) {
        let halo = Rgba::new(0, 0, 0, 190);
        let white = Rgba::rgb(255, 255, 255);

        let hx = if dx > 0 {
            corner_x
        } else {
            corner_x - leg + thick
        };
        let hy = if dy > 0 { corner_y } else { corner_y };

        let vx = if dx > 0 { corner_x } else { corner_x };
        let vy = if dy > 0 {
            corner_y
        } else {
            corner_y - leg + thick
        };

        self.fill_rect_i32(hx - 1, hy - 1, leg + 2, thick + 2, halo);
        self.fill_rect_i32(vx - 1, vy - 1, thick + 2, leg + 2, halo);

        self.fill_rect_i32(hx, hy, leg, thick, white);
        self.fill_rect_i32(vx, vy, thick, leg, white);
    }
}
