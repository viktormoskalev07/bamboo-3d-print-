use crate::config::{Config, BED_H, BED_W};
use crate::geom::{dist, Paths, Pt};
use anyhow::{bail, Result};
use std::fmt::Write;

/// Штрих в координатах стола (где должен быть КОНЧИК РУЧКИ), с высотой Z для «пера вниз».
pub struct Stroke {
    pub pts: Vec<Pt>,
    pub z: f64,
}

#[derive(Default, Clone, Copy)]
pub struct Stats {
    pub draw_mm: f64,
    pub travel_mm: f64,
    pub lifts: usize,
    pub secs: f64,
}

/// Координаты рабочей области листа (мм, Y вниз) -> координаты стола (Y вверх).
pub fn page_to_bed(mut paths: Paths, cfg: &Config) -> Paths {
    let (ox, oy) = cfg.paper_origin();
    let m = cfg.paper.margin;
    let top = oy + cfg.paper.height - m;
    crate::geom::map(&mut paths, |(x, y)| (ox + m + x, top - y));
    paths
}

pub fn strokes(paths: Paths, z: f64) -> Vec<Stroke> {
    paths.into_iter().map(|pts| Stroke { pts, z }).collect()
}

pub fn check_bounds(strokes: &[Stroke], cfg: &Config) -> Result<()> {
    let p = &cfg.pen;
    for s in strokes {
        for &(x, y) in &s.pts {
            let (nx, ny) = ((x - p.offset_x) * p.scale_x, (y - p.offset_y) * p.scale_y);
            if !(0.0..=BED_W).contains(&nx) || !(0.0..=BED_H).contains(&ny) {
                bail!(
                    "точка ({x:.1}, {y:.1}) недостижима для ручки: соплу нужно ({nx:.1}, {ny:.1}), а стол 0..{BED_W}.\n\
                     Сдвиньте лист (Бумага → Левый край X / Нижний край Y), уменьшите его или проверьте смещение ручки."
                );
            }
        }
    }
    Ok(())
}

struct Gen<'a> {
    cfg: &'a Config,
    emit: bool,
    out: String,
    t: f64,
    total: f64,
    pct: i32,
    pos: Pt,
    z: f64,
    st: Stats,
}

impl Gen<'_> {
    fn line(&mut self, s: &str) {
        if self.emit {
            self.out.push_str(s);
            self.out.push('\n');
        }
    }
    /// Кончик ручки (на столе) -> координаты сопла в системе принтера (с учётом инверсии осей).
    fn nozzle(&self, p: Pt) -> Pt {
        let pen = &self.cfg.pen;
        let (x, y) = ((p.0 - pen.offset_x) * pen.scale_x, (p.1 - pen.offset_y) * pen.scale_y);
        (if pen.invert_x { BED_W - x } else { x }, if pen.invert_y { BED_H - y } else { y })
    }
    fn mv(&mut self, to: Pt, draw: bool) {
        let n = self.nozzle(to);
        let d = dist(self.pos, n);
        let speed = if draw { self.cfg.pen.draw_speed } else { self.cfg.pen.travel_speed };
        self.t += d / speed + 0.01;
        if draw {
            self.st.draw_mm += d;
        } else {
            self.st.travel_mm += d;
        }
        self.pos = n;
        let s = format!("G1 X{:.3} Y{:.3} F{:.0}", n.0, n.1, speed * 60.0);
        self.line(&s);
    }
    fn z(&mut self, z: f64) {
        if (z - self.z).abs() < 1e-6 {
            return;
        }
        self.t += (z - self.z).abs() / self.cfg.pen.z_speed + 0.05;
        self.z = z;
        let s = format!("G1 Z{:.3} F{:.0}", z, self.cfg.pen.z_speed * 60.0);
        self.line(&s);
    }
    fn progress(&mut self) {
        if !self.emit || self.total <= 0.0 {
            return;
        }
        let p = ((self.t / self.total * 100.0).floor() as i32).min(99);
        if p > self.pct {
            self.pct = p;
            let r = ((self.total - self.t) / 60.0).ceil().max(0.0);
            let s = format!("M73 P{p} R{r:.0}");
            self.line(&s);
        }
    }

    fn run(&mut self, strokes: &[Stroke], title: &str) {
        let pen = self.cfg.pen.clone();
        let total_min = (self.total / 60.0).ceil();
        let title: String = title.chars().map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '_' }).collect();
        let mut head = String::new();
        let _ = writeln!(head, "; HEADER_BLOCK_START");
        let _ = writeln!(head, "; BambuStudio 02.08.02.61 (BambuPlotter: {title})");
        let _ = writeln!(head, "; model printing time: {:.0}s; total estimated time: {:.0}s", self.total, self.total);
        let _ = writeln!(head, "; total layer number: 1");
        let _ = writeln!(head, "; total filament length [mm] : 0.00");
        let _ = writeln!(head, "; total filament volume [cm^3] : 0.00");
        let _ = writeln!(head, "; total filament weight [g] : 0.00");
        let _ = writeln!(head, "; filament_density: 1.24");
        let _ = writeln!(head, "; filament_diameter: 1.75");
        let _ = writeln!(head, "; max_z_height: {:.2}", pen.z_up);
        let _ = writeln!(head, "; filament: 1");
        let _ = writeln!(head, "; support_material_on_wipe_tower: 0");
        let _ = writeln!(head, "; HEADER_BLOCK_END");
        let _ = writeln!(head, "M73 P0 R{total_min:.0}");
        let _ = writeln!(head, "M412 S0 ; filament runout detection off (pen plotter)");
        let _ = writeln!(head, "G392 S0 ; clog detection off");
        let _ = writeln!(head, "M104 S0 ; no nozzle heating");
        let _ = writeln!(head, "M140 S0 ; no bed heating");
        let _ = writeln!(head, "M106 S0");
        let _ = writeln!(head, "G90");
        let _ = writeln!(head, "G21");
        let _ = writeln!(head, "M18 E ; extruder motor off");
        let _ = write!(head, "M83");
        self.line(&head);
        if pen.home {
            self.line("G28");
            self.z = 0.0;
            self.pos = (0.0, 0.0);
        }
        self.line(&format!("G1 Z{:.3} F{:.0}", pen.z_up, pen.z_speed * 60.0));
        self.t += 1.0;
        self.z = pen.z_up;

        let mut down = false;
        for s in strokes {
            let start = s.pts[0];
            let connected = down && dist(self.pos, self.nozzle(start)) < 0.05 && (self.z - s.z).abs() < 1e-6;
            if !connected {
                if down {
                    self.z(pen.z_up);
                }
                self.mv(start, false);
                self.z(s.z);
                self.st.lifts += 1;
                if pen.pen_down_delay_ms > 0 {
                    self.line(&format!("G4 P{}", pen.pen_down_delay_ms));
                    self.t += pen.pen_down_delay_ms as f64 / 1000.0;
                }
                down = true;
            }
            for &p in &s.pts[1..] {
                self.mv(p, true);
            }
            self.progress();
        }
        self.z(pen.z_up);
        self.z((pen.z_up + 10.0).min(240.0));
        // парковка задана в координатах принтера -> переводим в «координаты ручки»
        let px = if pen.invert_x { BED_W - pen.park_x } else { pen.park_x };
        let py = if pen.invert_y { BED_H - pen.park_y } else { pen.park_y };
        let park = (px / pen.scale_x + pen.offset_x, py / pen.scale_y + pen.offset_y);
        self.mv(park, false);
        self.line("M400");
        self.line("M104 S0 ; heaters stay off");
        self.line("M140 S0");
        self.line("M73 P100 R0");
        self.line("; end");
        self.st.secs = self.t;
    }
}

/// Отрезок движения кончика ручки, восстановленный из G-code.
#[derive(Clone, Copy)]
pub struct Seg {
    pub a: Pt,
    pub b: Pt,
    /// ручка касается бумаги (Z ниже середины между z_down и z_up)
    pub down: bool,
}

/// Разбор готового G-code: что реально сделает принтер (координаты кончика ручки на столе).
pub fn simulate(code: &str, cfg: &Config) -> Vec<Seg> {
    let (ox, oy) = (cfg.pen.offset_x, cfg.pen.offset_y);
    let (ix, iy) = (cfg.pen.invert_x, cfg.pen.invert_y);
    let (fx, fy) = (cfg.pen.scale_x, cfg.pen.scale_y);
    let unflip = |x: f64, y: f64| -> Pt { ((if ix { BED_W - x } else { x }) / fx + ox, (if iy { BED_H - y } else { y }) / fy + oy) };
    let touch = (cfg.pen.z_down + cfg.pen.z_up) / 2.0;
    let (mut x, mut y, mut z) = (0.0f64, 0.0f64, f64::MAX);
    let mut abs = true;
    let mut out = Vec::new();
    for line in code.lines() {
        let l = line.split(';').next().unwrap_or("").trim();
        let mut words = l.split_whitespace();
        let Some(cmd) = words.next() else { continue };
        match cmd {
            "G90" => abs = true,
            "G91" => abs = false,
            "G28" => {
                x = 0.0;
                y = 0.0;
                z = 0.0;
            }
            "G0" | "G1" => {
                let (mut nx, mut ny, mut nz) = (x, y, z);
                for w in words {
                    let v = w.get(1..).and_then(|s| s.parse::<f64>().ok());
                    match (w.as_bytes()[0], v) {
                        (b'X', Some(v)) => nx = if abs { v } else { x + v },
                        (b'Y', Some(v)) => ny = if abs { v } else { y + v },
                        (b'Z', Some(v)) => nz = if abs { v } else { z + v },
                        _ => {}
                    }
                }
                if (nx - x).abs() > 1e-9 || (ny - y).abs() > 1e-9 {
                    out.push(Seg { a: unflip(x, y), b: unflip(nx, ny), down: z < touch && nz < touch });
                }
                x = nx;
                y = ny;
                z = nz;
            }
            _ => {}
        }
    }
    out
}

/// Только оценка (время, длины) без генерации текста G-code.
#[allow(dead_code)]
pub fn estimate(strokes: &[Stroke], cfg: &Config) -> Stats {
    let mut g = Gen {
        cfg,
        emit: false,
        out: String::new(),
        t: 0.0,
        total: 0.0,
        pct: 0,
        pos: (0.0, 0.0),
        z: 0.0,
        st: Stats::default(),
    };
    g.run(strokes, "");
    g.st
}

pub fn build(strokes: &[Stroke], cfg: &Config, title: &str) -> (String, Stats) {
    let mk = |emit, total| Gen {
        cfg,
        emit,
        out: String::new(),
        t: 0.0,
        total,
        pct: 0,
        pos: (0.0, 0.0),
        z: 0.0,
        st: Stats::default(),
    };
    let mut dry = mk(false, 0.0);
    dry.run(strokes, title);
    let total = dry.t;
    let mut g = mk(true, total);
    g.run(strokes, title);
    (g.out, g.st)
}
