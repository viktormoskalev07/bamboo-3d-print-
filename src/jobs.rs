//! Документ (что рисуем) -> страницы со штрихами -> файлы задания.

use crate::config::Config;
use crate::gcode::{self, Stats, Stroke};
use crate::geom::{self, Paths};
use crate::text::Font;
use crate::{output, table, text};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Text,
    Table,
    Image,
    Draw,
    Calibrate,
}

/// Инструменты вкладки «Рисование».
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// выбрать фигуру щелчком, перетащить, удалить клавишей Delete
    Select,
    Pencil,
    Line,
    Rect,
    Circle,
    Grid,
}

/// Фигура на листе. Координаты — мм от левого верхнего угла рабочей области, Y вниз.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum Shape {
    Free(Vec<(f64, f64)>),
    Line((f64, f64), (f64, f64)),
    Rect((f64, f64), (f64, f64)),
    Circle((f64, f64), f64),
    Grid { a: (f64, f64), b: (f64, f64), step: f64, labels: bool },
    /// Надпись: `at` — середина базовой линии.
    Text { at: (f64, f64), text: String, size: f64 },
}

impl Shape {
    /// Фигура по инструменту и двум точкам (начало/конец перетаскивания).
    pub fn from_drag(tool: Tool, pts: &[(f64, f64)], step: f64, labels: bool) -> Option<Shape> {
        let (&a, &b) = (pts.first()?, pts.last()?);
        let d = geom::dist(a, b);
        match tool {
            Tool::Select => None,
            Tool::Pencil => (pts.len() >= 2).then(|| Shape::Free(pts.to_vec())),
            Tool::Line => (d > 0.2).then_some(Shape::Line(a, b)),
            Tool::Rect => ((a.0 - b.0).abs() > 0.2 && (a.1 - b.1).abs() > 0.2).then_some(Shape::Rect(a, b)),
            Tool::Circle => (d > 0.2).then_some(Shape::Circle(a, d)),
            Tool::Grid => ((a.0 - b.0).abs() >= step && (a.1 - b.1).abs() >= step)
                .then_some(Shape::Grid { a, b, step: step.max(0.5), labels }),
        }
    }

    pub fn paths(&self, font: &Font) -> Paths {
        match self {
            Shape::Text { at, text, size } => {
                let mut out = Paths::new();
                let w = font.width(text, *size);
                font.draw(text, at.0 - w / 2.0, at.1, *size, &mut out);
                out
            }
            Shape::Free(p) => vec![p.clone()],
            Shape::Line(a, b) => vec![vec![*a, *b]],
            Shape::Rect(a, b) => vec![vec![*a, (b.0, a.1), *b, (a.0, b.1), *a]],
            Shape::Circle(c, r) => {
                let n = ((r * std::f64::consts::TAU / 0.5).ceil() as usize).clamp(24, 720);
                vec![(0..=n)
                    .map(|i| {
                        let t = i as f64 / n as f64 * std::f64::consts::TAU;
                        (c.0 + r * t.cos(), c.1 + r * t.sin())
                    })
                    .collect()]
            }
            Shape::Grid { a, b, step, labels } => {
                let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
                let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
                let nx = ((x1 - x0) / step + 1e-6).floor() as usize;
                let ny = ((y1 - y0) / step + 1e-6).floor() as usize;
                let (x1, y1) = (x0 + nx as f64 * step, y0 + ny as f64 * step);
                let mut out = Paths::new();
                // змейкой — меньше холостых переездов
                for i in 0..=ny {
                    let y = y0 + i as f64 * step;
                    out.push(if i % 2 == 0 { vec![(x0, y), (x1, y)] } else { vec![(x1, y), (x0, y)] });
                }
                for i in 0..=nx {
                    let x = x0 + i as f64 * step;
                    out.push(if i % 2 == 0 { vec![(x, y0), (x, y1)] } else { vec![(x, y1), (x, y0)] });
                }
                if *labels {
                    // подписи в мм: над верхней линией и слева от левой
                    let every = if *step >= 10.0 { 1 } else { (10.0 / step).ceil() as usize };
                    let size = (step * every as f64 * 0.3).clamp(1.5, 3.5);
                    for i in (0..=nx).step_by(every) {
                        let t = format!("{}", (i as f64 * step).round());
                        let w = font.width(&t, size);
                        font.draw(&t, x0 + i as f64 * step - w / 2.0, y0 - size * 0.5, size, &mut out);
                    }
                    for i in (every..=ny).step_by(every) {
                        let t = format!("{}", (i as f64 * step).round());
                        let w = font.width(&t, size);
                        font.draw(&t, x0 - w - size * 0.5, y0 + i as f64 * step + size / 2.0, size, &mut out);
                    }
                }
                out
            }
        }
    }

    /// Расстояние от точки до линий фигуры (мм) — для выбора мышью.
    pub fn distance_to(&self, p: (f64, f64), font: &Font) -> f64 {
        let seg = |a: (f64, f64), b: (f64, f64)| {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l2 = dx * dx + dy * dy;
            let t = if l2 < 1e-12 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) };
            geom::dist(p, (a.0 + dx * t, a.1 + dy * t))
        };
        let mut best = f64::MAX;
        for path in self.paths(font) {
            for w in path.windows(2) {
                best = best.min(seg(w[0], w[1]));
            }
        }
        // внутри прямоугольника/сетки/текста тоже считаем попаданием
        if let Some(b) = geom::bbox(&self.paths(font)) {
            if matches!(self, Shape::Grid { .. } | Shape::Text { .. }) && p.0 >= b[0] && p.0 <= b[2] && p.1 >= b[1] && p.1 <= b[3] {
                best = 0.0;
            }
        }
        best
    }

    pub fn translate(&mut self, dx: f64, dy: f64) {
        let mv = |p: &mut (f64, f64)| {
            p.0 += dx;
            p.1 += dy;
        };
        match self {
            Shape::Free(pts) => pts.iter_mut().for_each(mv),
            Shape::Line(a, b) | Shape::Rect(a, b) | Shape::Grid { a, b, .. } => {
                mv(a);
                mv(b);
            }
            Shape::Circle(c, _) => mv(c),
            Shape::Text { at, .. } => mv(at),
        }
    }

    /// Размеры фигуры для подсказки во время рисования.
    pub fn describe(&self) -> String {
        match self {
            Shape::Free(p) => {
                let len: f64 = p.windows(2).map(|w| geom::dist(w[0], w[1])).sum();
                format!("{len:.0} мм")
            }
            Shape::Line(a, b) => format!("{:.1} мм", geom::dist(*a, *b)),
            Shape::Rect(a, b) | Shape::Grid { a, b, .. } => {
                format!("{:.1} × {:.1} мм", (a.0 - b.0).abs(), (a.1 - b.1).abs())
            }
            Shape::Circle(_, r) => format!("Ø {:.1} мм", r * 2.0),
            Shape::Text { text, size, .. } => format!("«{text}» {size:.0} мм"),
        }
    }
}

/// Тест направления: по отпечатку видно, не отражён ли рисунок и где у листа «зад».
pub fn orientation_test(u: [f64; 4]) -> Vec<Shape> {
    let (w, h) = (u[2] - u[0], u[3] - u[1]);
    let cx = w / 2.0;
    let t = |x: f64, y: f64, s: &str, size: f64| Shape::Text { at: (x, y), text: s.to_string(), size };
    vec![
        // уголок в левом верхнем углу листа
        Shape::Free(vec![(0.0, 15.0), (0.0, 0.0), (15.0, 0.0)]),
        t(cx, 10.0, "ЗАД (к задней стенке)", 5.0),
        // стрелка вверх — к задней стенке
        Shape::Line((cx, h * 0.45 - 24.0), (cx, 22.0)),
        Shape::Free(vec![(cx - 7.0, 32.0), (cx, 22.0), (cx + 7.0, 32.0)]),
        t(cx, h * 0.45, "R F У", 18.0),
        t(14.0, h * 0.3, "ЛЕВО", 4.0),
        t(w - 16.0, h * 0.3, "ПРАВО", 4.0),
        t(cx, h * 0.75, "Если надписи читаются нормально,", 3.5),
        t(cx, h * 0.75 + 7.0, "а стрелка смотрит к задней стенке —", 3.5),
        t(cx, h * 0.75 + 14.0, "всё настроено верно.", 3.5),
        t(cx, h - 3.0, "ПЕРЕД (к вам)", 5.0),
    ]
    .into_iter()
    .map(|mut s| {
        s.translate(u[0], u[1]);
        s
    })
    .collect()
}

/// Тестовая сетка для проверки масштаба: квадрат 100×100 мм, клетка 10 мм, с подписями.
pub fn scale_test() -> Vec<Shape> {
    vec![Shape::Grid { a: (8.0, 8.0), b: (108.0, 108.0), step: 10.0, labels: true }]
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Doc {
    pub mode: Mode,
    pub job_name: String,
    pub text: String,
    pub align: u8,
    pub table: Vec<Vec<String>>,
    pub table_header: bool,
    pub svg_path: String,
    pub svg_fit: bool,
    pub svg_scale: f64,
    pub svg_rot: u8,
    pub cal_step: f64,
    pub cal_steps: u32,
    pub shapes: Vec<Shape>,
    pub tool: Tool,
    pub grid_step: f64,
    pub grid_labels: bool,
    /// Привязка к сетке, мм (0 — выкл.)
    pub snap: f64,

    // --- единый лист: что показывать и где ---
    pub show_text: bool,
    pub show_table: bool,
    pub show_image: bool,
    pub show_drawing: bool,
    /// левый верхний угол текста (мм от угла рабочей области)
    pub text_pos: (f64, f64),
    /// None — автоматически под текстом
    pub table_pos: Option<(f64, f64)>,
    /// None — автоматически под текстом и таблицей (или по центру, если «растянуть на лист»)
    pub svg_pos: Option<(f64, f64)>,
    /// параметры превращения фото/картинки (JPG, PNG…) в линии
    pub img: crate::raster::Params,
    /// обрезка картинки [u0, v0, u1, v1] в долях исходного размера
    pub img_crop: [f64; 4],
    /// мазки ластика по картинке
    pub img_erase: Vec<crate::raster::Erase>,
    /// автоматически убрать однотонный фон (заливка от краёв)
    pub img_bg: bool,
    pub img_bg_tol: f64,
}

/// Источник «Рисунка»: векторный SVG или растровая картинка.
pub enum ImageSrc {
    Vector(Paths),
    Raster(crate::raster::Raster),
}

/// Элемент листа (для выделения и перетаскивания мышью).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum Elem {
    Text,
    Table,
    Image,
    Drawing,
}

impl Elem {
    pub fn of_mode(m: Mode) -> Option<Elem> {
        match m {
            Mode::Text => Some(Elem::Text),
            Mode::Table => Some(Elem::Table),
            Mode::Image => Some(Elem::Image),
            Mode::Draw => Some(Elem::Drawing),
            Mode::Calibrate => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Elem::Text => "Текст",
            Elem::Table => "Таблица",
            Elem::Image => "Рисунок",
            Elem::Drawing => "Рисование",
        }
    }
}

impl Default for Doc {
    fn default() -> Self {
        Doc {
            mode: Mode::Text,
            job_name: "Мой рисунок".into(),
            text: "Привет! Это Bambu Plotter.\n\nПишите текст здесь — превью справа обновляется сразу.".into(),
            align: 0,
            table: vec![
                vec!["Товар".into(), "Кол-во".into(), "Цена".into()],
                vec!["Яблоки".into(), "3".into(), "120,50".into()],
                vec!["Груши".into(), "12".into(), "99".into()],
            ],
            table_header: true,
            svg_path: String::new(),
            svg_fit: true,
            svg_scale: 100.0,
            svg_rot: 0,
            cal_step: 0.25,
            cal_steps: 4,
            shapes: Vec::new(),
            tool: Tool::Pencil,
            grid_step: 10.0,
            grid_labels: true,
            snap: 1.0,
            show_text: true,
            show_table: false,
            show_image: true,
            show_drawing: true,
            text_pos: (0.0, 0.0),
            table_pos: None,
            svg_pos: None,
            img: crate::raster::Params::default(),
            img_crop: [0.0, 0.0, 1.0, 1.0],
            img_erase: Vec::new(),
            img_bg: false,
            img_bg_tol: 0.12,
        }
    }
}

pub struct Page {
    pub strokes: Vec<Stroke>,
    pub stats: Stats,
    pub error: Option<String>,
    /// то, что реально нарисует принтер — восстановлено из сгенерированного G-code
    pub segs: Vec<gcode::Seg>,
    /// габариты элементов на этой странице (мм рабочей области, Y вниз)
    pub boxes: Vec<(Elem, [f64; 4])>,
    /// часть рисунка вне зоны ручки — обрезана (печатать можно)
    pub warning: Option<String>,
    /// прямоугольник картинки на листе [x0, y0, x1, y1] (мм рабочей области) — для ластика/обрезки/размера
    pub image_rect: Option<[f64; 4]>,
}

fn shifted(mut p: Paths, (dx, dy): (f64, f64)) -> Paths {
    geom::map(&mut p, |(x, y)| (x + dx, y + dy));
    p
}

pub fn table_has_content(t: &[Vec<String>]) -> bool {
    t.iter().flatten().any(|c| !c.trim().is_empty())
}

/// Единый лист: текст + таблица + SVG + рисование одновременно.
/// Текст и таблица могут продолжаться на следующих страницах; SVG и рисование — на первой.
pub fn build_pages(doc: &Doc, cfg: &Config, font: &Font, svg: Option<&ImageSrc>) -> Vec<Page> {
    if doc.mode == Mode::Calibrate {
        let strokes = calibration(doc, cfg, font);
        return finish_pages(vec![(strokes, Vec::new())], cfg);
    }
    let (layers, image_rect) = layout_layers(doc, cfg, font, svg);
    let n = layers.iter().map(|(_, pages, _)| pages.len()).max().unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..n {
        let mut strokes = Vec::new();
        let mut boxes = Vec::new();
        for (elem, pages, ordered) in &layers {
            let Some(p) = pages.get(i) else { continue };
            if p.is_empty() {
                continue;
            }
            if let Some(b) = geom::bbox(p) {
                boxes.push((*elem, b));
            }
            strokes.extend(if *ordered { to_strokes_ordered(p.clone(), cfg) } else { to_strokes(p.clone(), cfg) });
        }
        out.push((strokes, boxes));
    }
    let mut pages = finish_pages(out, cfg);
    if let Some(p) = pages.first_mut() {
        p.image_rect = image_rect;
    }
    pages
}

/// Раскладка всех элементов в координатах рабочей области. Возвращает (положения, слои по страницам).
fn layout_layers(doc: &Doc, cfg: &Config, font: &Font, svg: Option<&ImageSrc>) -> (Vec<(Elem, Vec<Paths>, bool)>, Option<[f64; 4]>) {
    // раскладываем только там, куда дотягивается ручка (важно для A4, который больше стола)
    let u = cfg.usable_area();
    let (size, ls) = (cfg.text.size, cfg.text.line_spacing);
    let gap = 6.0;
    let mut layers: Vec<(Elem, Vec<Paths>, bool)> = Vec::new();
    let mut image_rect: Option<[f64; 4]> = None;
    let mut bottom: Option<f64> = None; // низ уже размещённого на первой странице
    let clamp_pos = |(x, y): (f64, f64)| (x.max(u[0]), y.max(u[1]));
    let room = |(x, y): (f64, f64)| ((u[2] - x).max(10.0), (u[3] - y).max(10.0));

    // текст — в порядке чтения
    if doc.show_text && !doc.text.trim().is_empty() {
        let pos = clamp_pos(doc.text_pos);
        let pages: Vec<Paths> =
            text::layout(font, &doc.text, size, ls, room(pos), doc.align).into_iter().map(|p| shifted(p, pos)).collect();
        if let Some(b) = pages.first().and_then(geom::bbox) {
            bottom = Some(bottom.unwrap_or(f64::MIN).max(b[3]));
        }
        layers.push((Elem::Text, pages, true));
    }

    // таблица — по умолчанию под текстом
    let below = |bottom: Option<f64>| (u[0], bottom.map_or(u[1], |b| b + gap));
    if doc.show_table && table_has_content(&doc.table) {
        let pos = doc.table_pos.unwrap_or_else(|| below(bottom));
        let pages: Vec<Paths> = table::layout(font, &doc.table, size, ls, room(clamp_pos(pos)), doc.table_header)
            .into_iter()
            .map(|p| shifted(p, pos))
            .collect();
        if let Some(b) = pages.first().and_then(geom::bbox) {
            bottom = Some(bottom.unwrap_or(f64::MIN).max(b[3]));
        }
        layers.push((Elem::Table, pages, true));
    }

    // SVG — по умолчанию под текстом и таблицей, вписан в оставшееся место
    if let (true, Some(src)) = (doc.show_image, svg) {
        let auto = below(bottom);
        match src {
            ImageSrc::Vector(p) => {
                let mut pos = doc.svg_pos.unwrap_or(auto);
                let room_auto = room(auto);
                let placed = place_image(p.clone(), doc, (room_auto.0 + auto.0, room_auto.1 + auto.1), doc.svg_pos.is_none().then_some(auto), &mut pos);
                image_rect = geom::bbox(&placed);
                layers.push((Elem::Image, vec![placed], false));
            }
            ImageSrc::Raster(r) => {
                // размер: «растянуть» — вписать в свободное место; иначе ширина 100 мм × масштаб
                let a0 = r.aspect_cropped(doc.img_crop);
                let aspect = if doc.svg_rot % 2 == 1 { 1.0 / a0 } else { a0 };
                let pos = doc.svg_pos.unwrap_or(auto);
                let (rw, rh) = room(if doc.svg_pos.is_some() { clamp_pos(pos) } else { auto });
                let (mut w, mut h) = if doc.svg_fit {
                    if rw / rh > aspect { (rh * aspect, rh) } else { (rw, rw / aspect) }
                } else {
                    let w = 100.0 * doc.svg_scale / 100.0;
                    (w, w / aspect)
                };
                if doc.svg_pos.is_none() && (w > rw || h > rh) {
                    let k = (rw / w).min(rh / h);
                    w *= k;
                    h *= k;
                }
                let lines = crate::raster::vectorize(r, (pos.0, pos.1, w, h), doc.svg_rot, doc.img_crop, &doc.img);
                image_rect = Some([pos.0, pos.1, pos.0 + w, pos.1 + h]);
                // штриховка и волны уже идут змейкой — порядок не перемешиваем
                let ordered = doc.img.style != crate::raster::Style::Contour;
                layers.push((Elem::Image, vec![lines], ordered));
            }
        }
    }

    // рисование — в абсолютных мм
    if doc.show_drawing && !doc.shapes.is_empty() {
        let p: Paths = doc.shapes.iter().flat_map(|s| s.paths(font)).collect();
        layers.push((Elem::Drawing, vec![p], false));
    }
    (layers, image_rect)
}

fn finish_pages(raw: Vec<(Vec<Stroke>, Vec<(Elem, [f64; 4])>)>, cfg: &Config) -> Vec<Page> {
    raw.into_iter()
        .filter(|(s, _)| !s.is_empty())
        .map(|(strokes, boxes)| {
            // всё, что вне зоны ручки, обрезаем — не блокируем печать
            let reach = cfg.pen_reach();
            let mut clipped = false;
            let mut kept = Vec::with_capacity(strokes.len());
            for st in strokes {
                let (parts, cut) = geom::clip(vec![st.pts], reach);
                clipped |= cut;
                kept.extend(parts.into_iter().map(|pts| Stroke { pts, z: st.z }));
            }
            let strokes = kept;
            let warning = clipped.then(|| "Часть рисунка выходит за зону, куда дотягивается ручка, — она обрезана и не будет напечатана.".to_string());
            let error = if strokes.is_empty() {
                Some("Всё содержимое листа вне зоны ручки — сдвиньте лист или элементы.".to_string())
            } else {
                gcode::check_bounds(&strokes, cfg).err().map(|e| e.to_string())
            };
            let (code, stats) = gcode::build(&strokes, cfg, "");
            let segs = gcode::simulate(&code, cfg);
            Page { strokes, stats, error, segs, boxes, warning, image_rect: None }
        })
        .collect()
}

fn to_strokes_ordered(p: Paths, cfg: &Config) -> Vec<Stroke> {
    let p = geom::clean(p);
    let bed = gcode::page_to_bed(p, cfg);
    gcode::strokes(geom::orient(bed, (cfg.pen.offset_x, cfg.pen.offset_y)), cfg.pen.z_down)
}

fn to_strokes(p: Paths, cfg: &Config) -> Vec<Stroke> {
    let p = geom::clean(p);
    let bed = gcode::page_to_bed(p, cfg);
    gcode::strokes(geom::optimize(bed, (cfg.pen.offset_x, cfg.pen.offset_y)), cfg.pen.z_down)
}

/// Размещение SVG. `auto` — автоположение (под другими элементами): рисунок вписывается в остаток листа.
/// Иначе — ставится левым верхним углом в `pos`. В `pos` возвращается фактический угол.
fn place_image(mut p: Paths, doc: &Doc, (aw, ah): (f64, f64), auto: Option<(f64, f64)>, pos: &mut (f64, f64)) -> Paths {
    for _ in 0..doc.svg_rot % 4 {
        geom::map(&mut p, |(x, y)| (-y, x));
    }
    let (px, py) = auto.unwrap_or(*pos);
    let (rw, rh) = ((aw - px).max(10.0), (ah - py).max(10.0));
    if doc.svg_fit {
        geom::fit(&mut p, rw, rh, true);
    } else {
        let s = doc.svg_scale / 100.0;
        geom::map(&mut p, |(x, y)| (x * s, y * s));
        if let Some(b) = geom::bbox(&p) {
            geom::map(&mut p, |(x, y)| (x - b[0], y - b[1]));
            if auto.is_some() && (b[2] - b[0] > rw || b[3] - b[1] > rh) {
                geom::fit(&mut p, rw, rh, false);
            }
        }
    }
    // fit() центрирует внутри (rw, rh); переносим так, чтобы угол рисунка был в (px, py)
    if let Some(b) = geom::bbox(&p) {
        let (dx, dy) = if auto.is_some() && doc.svg_fit { (px, py) } else { (px - b[0], py - b[1]) };
        geom::map(&mut p, |(x, y)| (x + dx, y + dy));
    }
    if let Some(b) = geom::bbox(&p) {
        *pos = (b[0], b[1]);
    }
    p
}

pub fn calibration_levels(doc: &Doc, cfg: &Config) -> Vec<f64> {
    let n = doc.cal_steps as i32;
    (-n..=n).rev().map(|i| cfg.pen.z_down + i as f64 * doc.cal_step).filter(|z| *z >= 0.0).collect()
}

fn calibration(doc: &Doc, cfg: &Config, font: &Font) -> Vec<Stroke> {
    let u = cfg.usable_area();
    let (aw, ah) = (u[2] - u[0], u[3] - u[1]);
    let levels = calibration_levels(doc, cfg);
    let row = (ah / levels.len().max(1) as f64).min(14.0);
    let size = (row * 0.45).min(5.0);
    let mut all = Vec::new();
    for (i, z) in levels.iter().enumerate() {
        let y = (i as f64 + 0.7) * row;
        let mut p: Paths = Vec::new();
        font.draw(&format!("{z:.2}"), 0.0, y + size * 0.35, size, &mut p);
        let lx = font.width("00.00 ", size);
        p.push(vec![(lx, y), (aw, y)]);
        for k in 0..6 {
            let x = lx + k as f64 * 3.0;
            p.push(vec![(x, y - row * 0.25), (x, y + row * 0.25)]);
        }
        all.extend(gcode::strokes(gcode::page_to_bed(shifted(p, (u[0], u[1])), cfg), *z));
    }
    all
}

pub fn export(page: &Page, cfg: &Config, path: &Path, title: &str) -> Result<()> {
    if let Some(e) = &page.error {
        anyhow::bail!("{e}");
    }
    let (code, st) = gcode::build(&page.strokes, cfg, title);
    let thumb = output::thumbnail(&page.strokes, cfg)?;
    // габариты рисунка в координатах сопла
    let all: Paths = page.strokes.iter().map(|s| s.pts.clone()).collect();
    let b = geom::bbox(&all).unwrap_or([0.0; 4]);
    let (ox, oy) = (cfg.pen.offset_x, cfg.pen.offset_y);
    let bbox = [b[0] - ox, b[1] - oy, b[2] - ox, b[3] - oy];
    output::write_3mf_ex(path, &code, &thumb, st.secs, bbox)
}
