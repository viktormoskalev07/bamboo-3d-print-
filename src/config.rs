use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Рабочая область стола A1 (мм).
pub const BED_W: f64 = 256.0;
pub const BED_H: f64 = 256.0;

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Config {
    pub printer: PrinterCfg,
    pub pen: PenCfg,
    pub paper: PaperCfg,
    pub text: TextCfg,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct PrinterCfg {
    /// "connect" — через Bambu Connect, "lan" — напрямую по сети
    pub method: String,
    pub ip: String,
    pub serial: String,
    pub access_code: String,
}

impl Default for PrinterCfg {
    fn default() -> Self {
        PrinterCfg { method: "connect".into(), ip: String::new(), serial: String::new(), access_code: String::new() }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct PenCfg {
    pub z_up: f64,
    pub z_down: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub draw_speed: f64,
    pub travel_speed: f64,
    pub z_speed: f64,
    pub pen_down_delay_ms: u32,
    pub home: bool,
    /// ось X принтера направлена наоборот (зеркальный отпечаток слева-направо)
    pub invert_x: bool,
    /// ось Y принтера направлена наоборот (отпечаток перевёрнут сверху-вниз)
    pub invert_y: bool,
    /// коррекция масштаба по X / Y (1.0 = без коррекции); подбирается по тесту 100×100 мм
    pub scale_x: f64,
    pub scale_y: f64,
    pub park_x: f64,
    pub park_y: f64,
}

impl Default for PenCfg {
    fn default() -> Self {
        PenCfg {
            z_up: 8.0,
            z_down: 3.0,
            offset_x: 0.0,
            offset_y: 0.0,
            draw_speed: 40.0,
            travel_speed: 150.0,
            z_speed: 15.0,
            pen_down_delay_ms: 0,
            home: true,
            invert_x: false,
            invert_y: false,
            scale_x: 1.0,
            scale_y: 1.0,
            park_x: 0.0,
            park_y: 250.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct PaperCfg {
    pub width: f64,
    pub height: f64,
    pub margin: f64,
    pub origin_x: Option<f64>,
    pub origin_y: Option<f64>,
}

impl Default for PaperCfg {
    fn default() -> Self {
        PaperCfg { width: 148.0, height: 210.0, margin: 10.0, origin_x: None, origin_y: None }
    }
}

impl Config {
    /// Зона, куда может дотянуться кончик ручки (с учётом смещения от сопла), в пределах стола:
    /// [x0, y0, x1, y1].
    pub fn pen_reach(&self) -> [f64; 4] {
        let (ox, oy) = (self.pen.offset_x, self.pen.offset_y);
        let (fx, fy) = (self.pen.scale_x.max(0.5), self.pen.scale_y.max(0.5));
        [ox.max(0.0), oy.max(0.0), (BED_W / fx + ox).min(BED_W), (BED_H / fy + oy).min(BED_H)]
    }

    /// Левый нижний угол листа на столе. «По центру» — по центру зоны досягаемости ручки.
    /// Лист больше стола (A4) может свисать за край.
    pub fn paper_origin(&self) -> (f64, f64) {
        let r = self.pen_reach();
        let p = &self.paper;
        let mut cx = (r[0] + r[2] - p.width) / 2.0;
        let mut cy = (r[1] + r[3] - p.height) / 2.0;
        if p.width <= BED_W {
            cx = cx.clamp(0.0, BED_W - p.width);
        }
        if p.height <= BED_H {
            cy = cy.clamp(0.0, BED_H - p.height);
        }
        (p.origin_x.unwrap_or(cx), p.origin_y.unwrap_or(cy))
    }

    /// Часть рабочей области листа, куда реально дотягивается ручка:
    /// [x0, y0, x1, y1] в мм от левого верхнего угла рабочей области (Y вниз).
    pub fn usable_area(&self) -> [f64; 4] {
        let (ox, oy) = self.paper_origin();
        let p = &self.paper;
        let m = p.margin;
        let (left, right, top, bottom) = (ox + m, ox + p.width - m, oy + p.height - m, oy + m);
        let r = self.pen_reach();
        let (x0, x1) = (left.max(r[0]), right.min(r[2]));
        let (yt, yb) = (top.min(r[3]), bottom.max(r[1]));
        [x0 - left, top - yt, (x1 - left).max(x0 - left), (top - yb).max(top - yt)]
    }
}

impl PaperCfg {
    /// Размер области для рисования (лист минус поля).
    pub fn area(&self) -> (f64, f64) {
        ((self.width - 2.0 * self.margin).max(1.0), (self.height - 2.0 * self.margin).max(1.0))
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct TextCfg {
    pub font: String,
    pub size: f64,
    pub line_spacing: f64,
}

impl Default for TextCfg {
    fn default() -> Self {
        TextCfg { font: String::new(), size: 6.0, line_spacing: 1.35 }
    }
}

/// Конфиг лежит рядом с exe.
pub fn path() -> PathBuf {
    match std::env::current_exe() {
        Ok(exe) => exe.with_file_name("plotter.toml"),
        Err(_) => PathBuf::from("plotter.toml"),
    }
}

pub fn load(path: &Path) -> Config {
    std::fs::read_to_string(path).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
}

pub fn to_string(cfg: &Config) -> String {
    toml::to_string_pretty(cfg).unwrap_or_default()
}

/// Атомарная запись: во временный файл, затем замена — файл никогда не остаётся полузаписанным.
pub fn save(path: &Path, s: &str) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, s)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
