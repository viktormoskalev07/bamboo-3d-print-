//! Растровые картинки (JPG, PNG, BMP, GIF, WebP) -> линии для пера.
//! Стили: штриховка (фото), волны (портреты, почти одна линия), контур (логотипы, рисунки).

use crate::geom::{dist, Paths, Pt};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];

pub fn is_raster(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Hatch,
    Waves,
    Contour,
}

/// Параметры векторизации (хранятся в документе).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct Params {
    pub style: Style,
    /// расстояние между линиями, мм
    pub spacing: f64,
    /// яркость / порог: -1..1 (больше — светлее, меньше линий)
    pub brightness: f64,
    /// контраст: 0.2..3
    pub contrast: f64,
    pub invert: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { style: Style::Hatch, spacing: 0.8, brightness: 0.0, contrast: 1.2, invert: false }
    }
}

/// Картинка в оттенках серого: 0 — белое (нет линий), 1 — чёрное.
#[derive(Clone)]
pub struct Raster {
    pub w: usize,
    pub h: usize,
    pub dark: Vec<f32>,
    /// уменьшенная копия RGBA для превью в интерфейсе
    pub thumb: (usize, usize, Vec<u8>),
}

pub fn load(path: &Path) -> Result<Raster> {
    let img = image::open(path).with_context(|| format!("не удалось открыть картинку {}", path.display()))?;
    // рабочее разрешение: до 1000 px по большей стороне
    let img = if img.width().max(img.height()) > 1000 { img.thumbnail(1000, 1000) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    let mut dark = Vec::with_capacity(w * h);
    for p in rgba.pixels() {
        let [r, g, b, a] = p.0;
        let lum = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
        // прозрачное считаем белым
        let a = a as f32 / 255.0;
        dark.push((1.0 - lum) * a);
    }
    let t = image::DynamicImage::ImageRgba8(rgba).thumbnail(256, 256).to_rgba8();
    let thumb = (t.width() as usize, t.height() as usize, t.into_raw());
    Ok(Raster { w, h, dark, thumb })
}

impl Raster {
    pub fn aspect(&self) -> f64 {
        self.w as f64 / self.h.max(1) as f64
    }

    /// Темнота в точке (u, v) ∈ [0,1]² с учётом яркости/контраста/инверсии (билинейно).
    fn sample(&self, u: f64, v: f64, p: &Params) -> f64 {
        let x = (u * (self.w as f64 - 1.0)).clamp(0.0, self.w as f64 - 1.0);
        let y = (v * (self.h as f64 - 1.0)).clamp(0.0, self.h as f64 - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let g = |xx: usize, yy: usize| self.dark[yy * self.w + xx] as f64;
        let d = g(x0, y0) * (1.0 - fx) * (1.0 - fy) + g(x1, y0) * fx * (1.0 - fy) + g(x0, y1) * (1.0 - fx) * fy + g(x1, y1) * fx * fy;
        let d = if p.invert { 1.0 - d } else { d };
        ((d - 0.5) * p.contrast + 0.5 - p.brightness * 0.5).clamp(0.0, 1.0)
    }
}

/// Векторизация в прямоугольник (x, y, w, h) мм (Y вниз). `rot` — поворот на 90°×rot.
pub fn vectorize(r: &Raster, rect: (f64, f64, f64, f64), rot: u8, p: &Params) -> Paths {
    let (rx, ry, rw, rh) = rect;
    let rot = rot % 4;
    // точка листа -> координаты картинки (u, v) с учётом поворота
    let uv = |x: f64, y: f64| -> (f64, f64) {
        let (a, b) = ((x - rx) / rw, (y - ry) / rh);
        match rot {
            1 => (b, 1.0 - a),
            2 => (1.0 - a, 1.0 - b),
            3 => (1.0 - b, a),
            _ => (a, b),
        }
    };
    let dark = |x: f64, y: f64| {
        let (u, v) = uv(x, y);
        r.sample(u, v, p)
    };
    let spacing = p.spacing.clamp(0.2, 10.0);
    match p.style {
        Style::Hatch => hatch(rect, spacing, &dark),
        Style::Waves => waves(rect, spacing, &dark),
        Style::Contour => contour(rect, spacing, &dark),
    }
}

/// Штриховка: слои под 0°, 90°, 45°, 135°; каждый следующий — только в более тёмных местах.
fn hatch(rect: (f64, f64, f64, f64), spacing: f64, dark: &dyn Fn(f64, f64) -> f64) -> Paths {
    let (rx, ry, rw, rh) = rect;
    let mut out = Paths::new();
    let layers = [(0.0f64, 0.12), (90.0, 0.35), (45.0, 0.58), (135.0, 0.8)];
    let step = (spacing * 0.35).clamp(0.15, 0.6);
    let (cx, cy) = (rx + rw / 2.0, ry + rh / 2.0);
    let half_diag = (rw * rw + rh * rh).sqrt() / 2.0;
    for (li, (angle, thr)) in layers.iter().enumerate() {
        let (s, c) = angle.to_radians().sin_cos();
        // направление линии (c, s), нормаль (-s, c)
        let n = (half_diag / spacing).ceil() as i64;
        for k in -n..=n {
            let off = k as f64 * spacing + if li % 2 == 1 { spacing / 2.0 } else { 0.0 };
            let mut seg: Vec<Pt> = Vec::new();
            let mut pts_line: Vec<Paths> = Vec::new();
            let mut t = -half_diag;
            let mut run = Paths::new();
            while t <= half_diag {
                let x = cx + c * t - s * off;
                let y = cy + s * t + c * off;
                let inside = x >= rx && x <= rx + rw && y >= ry && y <= ry + rh;
                if inside && dark(x, y) > *thr {
                    seg.push((x, y));
                } else if seg.len() >= 2 {
                    run.push(std::mem::take(&mut seg));
                } else {
                    seg.clear();
                }
                t += step;
            }
            if seg.len() >= 2 {
                run.push(seg);
            }
            // змейкой: нечётные линии — в обратную сторону
            if k.rem_euclid(2) == 1 {
                run.reverse();
                for p in &mut run {
                    p.reverse();
                }
            }
            pts_line.push(run);
            for r in pts_line {
                for p in r {
                    // прямые отрезки: оставляем только концы
                    out.push(vec![p[0], *p.last().unwrap()]);
                }
            }
        }
    }
    out
}

/// Волны: горизонтальные строки-синусоиды; размах и частота растут с темнотой.
fn waves(rect: (f64, f64, f64, f64), spacing: f64, dark: &dyn Fn(f64, f64) -> f64) -> Paths {
    let (rx, ry, rw, rh) = rect;
    let mut out = Paths::new();
    let rows = (rh / spacing).floor().max(1.0) as usize;
    let step = (spacing * 0.12).clamp(0.08, 0.4);
    for i in 0..rows {
        let y0 = ry + (i as f64 + 0.5) * spacing;
        let mut phase = 0.0f64;
        let mut cur: Vec<Pt> = Vec::new();
        let mut row = Paths::new();
        let mut x = rx;
        while x <= rx + rw {
            let d = dark(x, y0);
            if d > 0.06 {
                let amp = d * spacing * 0.48;
                phase += (0.6 + d * 3.4) * std::f64::consts::PI * step / spacing.max(0.1) * 0.9;
                cur.push((x, y0 + amp * phase.sin()));
            } else if cur.len() >= 2 {
                row.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
            x += step;
        }
        if cur.len() >= 2 {
            row.push(cur);
        }
        if i % 2 == 1 {
            row.reverse();
            for p in &mut row {
                p.reverse();
            }
        }
        out.extend(row.into_iter().map(|p| simplify(&p, 0.02)));
    }
    out
}

/// Контур: границы тёмных областей (порог 0.5) методом marching squares.
fn contour(rect: (f64, f64, f64, f64), spacing: f64, dark: &dyn Fn(f64, f64) -> f64) -> Paths {
    let (rx, ry, rw, rh) = rect;
    let cell = (spacing * 0.5).clamp(0.15, 2.0);
    let nx = (rw / cell).ceil() as usize + 1;
    let ny = (rh / cell).ceil() as usize + 1;
    let mut v = vec![0.0f64; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let (x, y) = (rx + i as f64 * cell, ry + j as f64 * cell);
            // за краем — «белое», чтобы контуры замыкались
            v[j * nx + i] = if i == 0 || j == 0 || i == nx - 1 || j == ny - 1 { 0.0 } else { dark(x.min(rx + rw), y.min(ry + rh)) };
        }
    }
    let thr = 0.5;
    let at = |i: usize, j: usize| v[j * nx + i];
    let pos = |i: usize, j: usize| (rx + i as f64 * cell, ry + j as f64 * cell);
    let lerp = |a: Pt, b: Pt, va: f64, vb: f64| {
        let t = if (vb - va).abs() < 1e-9 { 0.5 } else { ((thr - va) / (vb - va)).clamp(0.0, 1.0) };
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    };
    let mut segs: Vec<(Pt, Pt)> = Vec::new();
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            let idx = (a > thr) as u8 | ((b > thr) as u8) << 1 | ((c > thr) as u8) << 2 | ((d > thr) as u8) << 3;
            if idx == 0 || idx == 15 {
                continue;
            }
            let (pa, pb, pc, pd) = (pos(i, j), pos(i + 1, j), pos(i + 1, j + 1), pos(i, j + 1));
            let top = lerp(pa, pb, a, b);
            let right = lerp(pb, pc, b, c);
            let bottom = lerp(pd, pc, d, c);
            let left = lerp(pa, pd, a, d);
            let mut add = |p: Pt, q: Pt| segs.push((p, q));
            match idx {
                1 | 14 => add(left, top),
                2 | 13 => add(top, right),
                3 | 12 => add(left, right),
                4 | 11 => add(right, bottom),
                6 | 9 => add(top, bottom),
                7 | 8 => add(left, bottom),
                5 => {
                    add(left, top);
                    add(right, bottom);
                }
                10 => {
                    add(top, right);
                    add(left, bottom);
                }
                _ => {}
            }
        }
    }
    join_segments(segs, cell * 0.01).into_iter().map(|p| simplify(&p, cell * 0.25)).filter(|p| p.len() >= 2).collect()
}

/// Склейка отрезков в ломаные по совпадающим концам.
fn join_segments(segs: Vec<(Pt, Pt)>, eps: f64) -> Paths {
    use std::collections::HashMap;
    let key = |p: Pt| ((p.0 / eps.max(1e-6)).round() as i64, (p.1 / eps.max(1e-6)).round() as i64);
    let mut ends: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (a, b)) in segs.iter().enumerate() {
        ends.entry(key(*a)).or_default().push(i);
        ends.entry(key(*b)).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut out = Paths::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut line = vec![segs[start].0, segs[start].1];
        // наращиваем в обе стороны
        for dir in 0..2 {
            loop {
                let tip = if dir == 0 { *line.last().unwrap() } else { line[0] };
                let next = ends.get(&key(tip)).and_then(|v| v.iter().copied().find(|&i| !used[i]));
                let Some(i) = next else { break };
                used[i] = true;
                let (a, b) = segs[i];
                let other = if key(a) == key(tip) { b } else { a };
                if dir == 0 {
                    line.push(other);
                } else {
                    line.insert(0, other);
                }
            }
        }
        out.push(line);
    }
    out
}

/// Упрощение ломаной (Рамер–Дуглас–Пекер).
fn simplify(p: &[Pt], eps: f64) -> Vec<Pt> {
    if p.len() < 3 {
        return p.to_vec();
    }
    let (a, b) = (p[0], *p.last().unwrap());
    let (mut idx, mut dmax) = (0, 0.0);
    for (i, &q) in p.iter().enumerate().take(p.len() - 1).skip(1) {
        let d = if dist(a, b) < 1e-12 {
            dist(a, q)
        } else {
            ((b.1 - a.1) * q.0 - (b.0 - a.0) * q.1 + b.0 * a.1 - b.1 * a.0).abs() / dist(a, b)
        };
        if d > dmax {
            idx = i;
            dmax = d;
        }
    }
    if dmax > eps {
        let mut l = simplify(&p[..=idx], eps);
        let r = simplify(&p[idx..], eps);
        l.pop();
        l.extend(r);
        l
    } else {
        vec![a, b]
    }
}
