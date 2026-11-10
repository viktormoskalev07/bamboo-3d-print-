use crate::geom::{dist, Paths, Pt};
use anyhow::{Context, Result};
use ttf_parser::{Face, GlyphId, OutlineBuilder};

use crate::hershey::Hershey;

/// Шрифт: встроенный однолинейный (data = None) или TTF/OTF-файл (рисуется контуром).
pub struct Font {
    data: Option<Vec<u8>>,
}

/// «Высота» шрифта = высота заглавной буквы в мм. У Hershey заглавная = 21 ед.
const H_UNIT: f64 = 1.0 / 21.0;

/// Межстрочный интервал 1.0 = 1.4 высоты заглавной (как обычный «одинарный» интервал).
pub const LINE_K: f64 = 1.4;

impl Font {
    pub fn default_name() -> &'static str {
        "Однолинейный (встроенный)"
    }

    /// Пустой путь — встроенный однолинейный шрифт.
    pub fn load(path: &str) -> Result<Font> {
        if path.is_empty() {
            return Ok(Font { data: None });
        }
        let data = std::fs::read(path).with_context(|| format!("шрифт не найден: {path}"))?;
        Face::parse(&data, 0).with_context(|| format!("не удалось разобрать шрифт {path}"))?;
        Ok(Font { data: Some(data) })
    }

    /// Масштаб TTF: высота заглавной буквы = size мм.
    fn ttf_scale(f: &Face, size: f64) -> f64 {
        let cap = f.capital_height().filter(|c| *c > 0).map(|c| c as f64).unwrap_or(f.units_per_em() as f64 * 0.716);
        size / cap
    }

    fn face(&self) -> Face<'_> {
        Face::parse(self.data.as_deref().expect("ttf font"), 0).expect("font checked on load")
    }

    pub fn ascender(&self, size: f64) -> f64 {
        if self.data.is_none() {
            return 26.0 * H_UNIT * size;
        }
        let f = self.face();
        f.ascender() as f64 * Self::ttf_scale(&f, size)
    }

    pub fn descender(&self, size: f64) -> f64 {
        if self.data.is_none() {
            return 7.0 * H_UNIT * size;
        }
        let f = self.face();
        -(f.descender() as f64) * Self::ttf_scale(&f, size)
    }

    fn glyph(face: &Face, c: char) -> Option<GlyphId> {
        face.glyph_index(c).or_else(|| face.glyph_index('?'))
    }

    fn advance(face: &Face, c: char) -> f64 {
        Self::glyph(face, c)
            .and_then(|g| face.glyph_hor_advance(g))
            .map(|a| a as f64)
            .unwrap_or(face.units_per_em() as f64 * 0.5)
    }

    pub fn width(&self, s: &str, size: f64) -> f64 {
        if self.data.is_none() {
            let h = Hershey::get();
            let sc = size * H_UNIT;
            return s.chars().map(|c| h.glyph(c).map_or(16.0, |g| g.right - g.left)).sum::<f64>() * sc;
        }
        let f = self.face();
        let sc = Self::ttf_scale(&f, size);
        s.chars().map(|c| Self::advance(&f, c)).sum::<f64>() * sc
    }

    /// Рисует строку: (x, baseline) — начало базовой линии, ось Y вниз.
    pub fn draw(&self, s: &str, x: f64, baseline: f64, size: f64, out: &mut Paths) {
        if self.data.is_none() {
            let h = Hershey::get();
            let sc = size * H_UNIT;
            let mut pen = x;
            for c in s.chars() {
                let Some(g) = h.glyph(c) else {
                    pen += 16.0 * sc;
                    continue;
                };
                if c != ' ' {
                    for st in &g.strokes {
                        let pts: Vec<Pt> =
                            st.iter().map(|&(gx, gy)| (pen + (gx - g.left) * sc, baseline + (gy - 9.0) * sc)).collect();
                        if pts.len() >= 2 {
                            out.push(pts);
                        } else if let Some(&p) = pts.first() {
                            out.push(vec![p, p]); // точка
                        }
                    }
                }
                pen += (g.right - g.left) * sc;
            }
            return;
        }
        let f = self.face();
        let sc = Self::ttf_scale(&f, size);
        let mut pen = 0.0;
        for c in s.chars() {
            if let Some(g) = Self::glyph(&f, c) {
                let mut b = Builder { sc, ox: x + pen * sc, oy: baseline, cur: Vec::new(), last: (0.0, 0.0), out };
                f.outline_glyph(g, &mut b);
                b.flush();
            }
            pen += Self::advance(&f, c);
        }
    }

    /// Перенос по словам под ширину `max_w`.
    pub fn wrap(&self, para: &str, size: f64, max_w: f64) -> Vec<String> {
        if para.trim().is_empty() {
            return vec![String::new()];
        }
        let mut lines = Vec::new();
        let mut cur = String::new();
        for word in para.split(' ') {
            let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if self.width(&cand, size) <= max_w {
                cur = cand;
                continue;
            }
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            cur = word.to_string();
            // слово длиннее строки — рубим по символам
            while self.width(&cur, size) > max_w && cur.chars().count() > 1 {
                let chars: Vec<char> = cur.chars().collect();
                let mut n = 1;
                while n < chars.len() && self.width(&chars[..=n].iter().collect::<String>(), size) <= max_w {
                    n += 1;
                }
                lines.push(chars[..n].iter().collect());
                cur = chars[n..].iter().collect();
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }
}

struct Builder<'a> {
    sc: f64,
    ox: f64,
    oy: f64,
    cur: Vec<Pt>,
    last: (f64, f64),
    out: &'a mut Paths,
}

impl Builder<'_> {
    fn map(&self, x: f64, y: f64) -> Pt {
        (self.ox + x * self.sc, self.oy - y * self.sc)
    }
    fn flush(&mut self) {
        if self.cur.len() >= 2 {
            self.out.push(std::mem::take(&mut self.cur));
        } else {
            self.cur.clear();
        }
    }
    fn steps(&self, ctrl: &[(f64, f64)]) -> usize {
        let len: f64 = ctrl.windows(2).map(|w| dist(w[0], w[1])).sum::<f64>() * self.sc;
        ((len / 0.3).ceil() as usize).clamp(2, 40)
    }
}

impl OutlineBuilder for Builder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.flush();
        let (x, y) = (x as f64, y as f64);
        let p = self.map(x, y);
        self.cur.push(p);
        self.last = (x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = (x as f64, y as f64);
        let p = self.map(x, y);
        self.cur.push(p);
        self.last = (x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p0 = self.last;
        let p1 = (x1 as f64, y1 as f64);
        let p2 = (x as f64, y as f64);
        let n = self.steps(&[p0, p1, p2]);
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            let px = u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0;
            let py = u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1;
            let p = self.map(px, py);
            self.cur.push(p);
        }
        self.last = p2;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p0 = self.last;
        let p1 = (x1 as f64, y1 as f64);
        let p2 = (x2 as f64, y2 as f64);
        let p3 = (x as f64, y as f64);
        let n = self.steps(&[p0, p1, p2, p3]);
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let u = 1.0 - t;
            let px = u * u * u * p0.0 + 3.0 * u * u * t * p1.0 + 3.0 * u * t * t * p2.0 + t * t * t * p3.0;
            let py = u * u * u * p0.1 + 3.0 * u * u * t * p1.1 + 3.0 * u * t * t * p2.1 + t * t * t * p3.1;
            let p = self.map(px, py);
            self.cur.push(p);
        }
        self.last = p3;
    }
    fn close(&mut self) {
        if let Some(&f) = self.cur.first() {
            self.cur.push(f);
        }
        self.flush();
    }
}

/// Раскладка текста по страницам. Координаты — мм внутри рабочей области, Y вниз.
/// `align`: 0 — влево, 1 — по центру, 2 — вправо.
pub fn layout(font: &Font, text: &str, size: f64, line_spacing: f64, area: (f64, f64), align: u8) -> Vec<Paths> {
    let (w, h) = area;
    let asc = font.ascender(size);
    let desc = font.descender(size);
    let lh = size * LINE_K * line_spacing;
    let mut lines = Vec::new();
    for para in text.replace('\t', "    ").replace('\r', "").split('\n') {
        lines.extend(font.wrap(para, size, w));
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let per_page = (((h - asc - desc) / lh).floor() as usize + 1).max(1);
    lines
        .chunks(per_page)
        .map(|chunk| {
            let mut out = Paths::new();
            for (i, l) in chunk.iter().enumerate() {
                let lw = font.width(l.trim_end(), size);
                let x = match align {
                    1 => (w - lw) / 2.0,
                    2 => w - lw,
                    _ => 0.0,
                };
                font.draw(l, x.max(0.0), asc + i as f64 * lh, size, &mut out);
            }
            out
        })
        .collect()
}
