use crate::config::{self, Config, BED_H, BED_W};
use crate::fonts::{self, FontEntry};
use crate::geom;
use crate::jobs::{self, Doc, Mode, Page};
use crate::text::Font;
use crate::{connect, printer, svg, table};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
    Vec2,
};
use crate::auto;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

// ---------- палитра ----------
const ACCENT: Color32 = Color32::from_rgb(0, 174, 66);
const ACCENT_DIM: Color32 = Color32::from_rgb(0, 110, 44);
const BG: Color32 = Color32::from_rgb(15, 17, 20);
const PANEL: Color32 = Color32::from_rgb(22, 25, 30);
const CARD: Color32 = Color32::from_rgb(30, 34, 41);
const CARD_HI: Color32 = Color32::from_rgb(40, 45, 54);
const DIM: Color32 = Color32::from_rgb(140, 147, 158);
const DANGER: Color32 = Color32::from_rgb(235, 80, 80);

const PAPER_PRESETS: &[(&str, f64, f64)] = &[
    ("A5 книжный", 148.0, 210.0),
    ("A5 альбомный", 210.0, 148.0),
    ("A6 книжный", 105.0, 148.0),
    ("A6 альбомный", 148.0, 105.0),
    ("Открытка 10×15", 100.0, 150.0),
    ("Квадрат 200×200", 200.0, 200.0),
    ("A4 книжный (свисает со стола)", 210.0, 297.0),
    ("A4 альбомный (свисает со стола)", 297.0, 210.0),
];

const HOTKEYS: &[(&str, &str)] = &[
    ("Ctrl+Z", "Отменить"),
    ("Ctrl+Shift+Z, Ctrl+Y", "Повторить"),
    ("Delete, Backspace", "Удалить выбранное"),
    ("Ctrl+C / Ctrl+V", "Копировать / вставить фигуру"),
    ("Ctrl+D", "Дублировать фигуру"),
    ("Стрелки (Shift)", "Сдвинуть выбранное на 1 мм (10 мм)"),
    ("Esc", "Снять выделение, закрыть окно"),
    ("Ctrl+O", "Открыть файл"),
    ("Ctrl+S", "Сохранить .gcode.3mf"),
    ("Ctrl+P", "Печать"),
    ("Ctrl+N", "Очистить лист"),
    ("Ctrl+= / Ctrl+−", "Приблизить / отдалить"),
    ("Ctrl+0", "Показать весь стол"),
    ("Ctrl+1…5", "Вкладки: Текст, Таблица, Рисунок, Рисование, Калибровка"),
    ("V", "Выбор (в «Рисовании»)"),
    ("B, P", "Карандаш"),
    ("L", "Линия"),
    ("U, R", "Прямоугольник"),
    ("E, O", "Круг"),
    ("G", "Сетка"),
    ("Колесо / правая кнопка", "Масштаб / сдвиг вида"),
    ("F1", "Эта справка"),
];

const DOC_KEY: &str = "doc";
const UI_KEY: &str = "ui";

#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(default)]
struct UiPrefs {
    ink: [u8; 3],
    show_travel: bool,
    show_settings: bool,
    sim_speed: f32,
    all_fonts: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        UiPrefs { ink: [28, 60, 170], show_travel: false, show_settings: true, sim_speed: 10.0, all_fonts: false }
    }
}

/// Перетаскивание элемента листа мышью.
struct Moving {
    elem: jobs::Elem,
    grab: (f64, f64),
    start: (f64, f64),
    applied: (f64, f64),
}

struct Toast {
    text: String,
    error: bool,
    until: f64,
}

pub struct App {
    doc: Doc,
    cfg: Config,
    cfg_path: PathBuf,
    cfg_saved: String,
    session_path: Option<PathBuf>,
    session_saved: String,
    save_error: Option<String>,
    saved_at: f64,
    prefs: UiPrefs,

    font: Option<Font>,
    font_loaded: Option<String>,
    font_list: Vec<FontEntry>,
    font_rx: Option<Receiver<Vec<FontEntry>>>,
    font_filter: String,
    all_fonts: bool,

    svg: Option<jobs::ImageSrc>,
    svg_loaded: String,
    img_tex: Option<egui::TextureHandle>,

    pages: Vec<Page>,
    page: usize,
    key: String,
    build_error: Option<String>,

    plate_tex: Option<egui::TextureHandle>,
    draw_pts: Vec<(f64, f64)>,
    moving: Option<Moving>,
    selected_shape: Option<usize>,
    selected_elem: Option<jobs::Elem>,
    shape_drag: Option<((f64, f64), (f64, f64))>,
    confirm_clear: bool,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    committed: Doc,
    prev_doc: Doc,
    last_edit: f64,
    shape_clip: Option<crate::jobs::Shape>,
    show_keys: bool,
    view_tf: Option<(Pos2, f32, f64, f64, f64, f64)>,
    meas_x: f64,
    meas_y: f64,
    zoom: f32,
    pan: Vec2,
    sim: f32,
    playing: bool,

    toast: Option<Toast>,
    job_rx: Option<Receiver<Result<String, String>>>,
    connect_ok: bool,
    connect_checked: f64,

    // автоматизация / тесты
    launch: auto::Launch,
    auto_rx: Option<Receiver<auto::Request>>,
    input_queue: VecDeque<Vec<egui::Event>>,
    after_input: Vec<Sender<Value>>,
    shots: Vec<(PathBuf, Option<Sender<Value>>)>,
    shot_wait: u32,
    cli_shot_started: bool,
    quit_after_shot: bool,
    frames: u64,
    shot_counter: u32,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: auto::Launch) -> Self {
        setup_style(&cc.egui_ctx);
        let persistent = launch.persistent();
        // тестовый запуск: копия настроек пользователя во временной папке, документ по умолчанию;
        // постоянный профиль (--profile): своя папка, сохраняется между запусками как обычный режим
        let cfg_path = if let Some(p) = &launch.profile {
            let dir = std::env::temp_dir().join(format!("bambu-plotter-profile-{p}"));
            let _ = std::fs::create_dir_all(&dir);
            dir.join("plotter.toml")
        } else if launch.is_test() {
            let dir = std::env::temp_dir().join("bambu-plotter-test");
            let _ = std::fs::create_dir_all(&dir);
            let p = dir.join("plotter.toml");
            let _ = std::fs::write(&p, config::to_string(&config::load(&config::path())));
            p
        } else {
            config::path()
        };
        let cfg = config::load(&cfg_path);
        // документ и вид — в session.json рядом с настройками (запись сразу при изменении);
        // если файла ещё нет — берём из старого хранилища egui
        let session_path = persistent.then(|| cfg_path.with_file_name("session.json"));
        let session: Option<Value> = session_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok());
        let mut doc: Doc = match (&session, persistent) {
            (Some(s), _) => serde_json::from_value(s["doc"].clone()).unwrap_or_default(),
            (None, true) => cc.storage.and_then(|s| eframe::get_value(s, DOC_KEY)).unwrap_or_default(),
            (None, false) => Doc::default(),
        };
        let prefs: UiPrefs = match (&session, persistent) {
            (Some(s), _) => serde_json::from_value(s["prefs"].clone()).unwrap_or_default(),
            (None, true) => cc.storage.and_then(|s| eframe::get_value(s, UI_KEY)).unwrap_or_default(),
            (None, false) => UiPrefs::default(),
        };
        let all_fonts = prefs.all_fonts;
        if let Some(m) = &launch.mode {
            if let Ok(m) = serde_json::from_value::<Mode>(json!(capitalize(m))) {
                doc.mode = m;
            }
        }
        let auto_rx = launch.automation.map(|port| auto::serve(port, cc.egui_ctx.clone()));
        let open_file = launch.open.clone();

        let (tx, rx) = channel();
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fonts::scan());
            ctx.request_repaint();
        });

        let mut app = App {
            doc,
            cfg_saved: config::to_string(&cfg),
            session_path,
            session_saved: String::new(),
            save_error: None,
            saved_at: -100.0,
            cfg,
            cfg_path,
            prefs,
            font: None,
            font_loaded: None,
            font_list: Vec::new(),
            font_rx: Some(rx),
            font_filter: String::new(),
            all_fonts,
            svg: None,
            svg_loaded: String::new(),
            img_tex: None,
            pages: Vec::new(),
            page: 0,
            key: String::new(),
            build_error: None,
            plate_tex: None,
            draw_pts: Vec::new(),
            moving: None,
            selected_shape: None,
            selected_elem: None,
            shape_drag: None,
            confirm_clear: false,
            undo: Vec::new(),
            redo: Vec::new(),
            committed: Doc::default(),
            prev_doc: Doc::default(),
            last_edit: 0.0,
            shape_clip: None,
            show_keys: false,
            view_tf: None,
            meas_x: 100.0,
            meas_y: 100.0,
            zoom: 1.0,
            pan: Vec2::ZERO,
            sim: 1.0,
            playing: false,
            toast: None,
            job_rx: None,
            connect_ok: connect::is_installed(),
            connect_checked: 0.0,
            launch,
            auto_rx,
            input_queue: VecDeque::new(),
            after_input: Vec::new(),
            shots: Vec::new(),
            shot_wait: 0,
            cli_shot_started: false,
            quit_after_shot: false,
            frames: 0,
            shot_counter: 0,
        };
        if let Some(f) = open_file {
            app.open_path(&f);
        }
        app.committed = app.doc.clone();
        app.prev_doc = app.doc.clone();
        app
    }

    // ---------------- логика ----------------

    fn notify(&mut self, ctx: &egui::Context, text: impl Into<String>, error: bool) {
        let now = ctx.input(|i| i.time);
        self.toast = Some(Toast { text: text.into(), error, until: now + if error { 9.0 } else { 5.0 } });
    }

    fn ensure_font(&mut self) {
        if self.font_loaded.as_deref() == Some(self.cfg.text.font.as_str()) {
            return;
        }
        match Font::load(&self.cfg.text.font) {
            Ok(f) => {
                self.font = Some(f);
                self.build_error = None;
            }
            Err(e) => {
                self.build_error = Some(format!("Шрифт: {e}"));
                if self.font.is_none() {
                    self.font = Font::load("").ok();
                }
            }
        }
        self.font_loaded = Some(self.cfg.text.font.clone());
    }

    fn ensure_svg(&mut self) {
        if self.svg_loaded == self.doc.svg_path {
            return;
        }
        self.svg_loaded = self.doc.svg_path.clone();
        self.svg = None;
        if self.doc.svg_path.is_empty() {
            return;
        }
        self.img_tex = None;
        let path = Path::new(&self.doc.svg_path);
        let loaded = if crate::raster::is_raster(path) {
            crate::raster::load(path).map(jobs::ImageSrc::Raster)
        } else {
            svg::load(path).map(jobs::ImageSrc::Vector)
        };
        match loaded {
            Ok(p) => self.svg = Some(p),
            Err(e) => self.build_error = Some(format!("SVG: {e:#}")),
        }
    }

    fn rebuild_if_needed(&mut self) {
        self.ensure_font();
        self.ensure_svg();
        let key = format!(
            "{}|{}|{}|{}",
            serde_json::to_string(&self.doc).unwrap_or_default(),
            config::to_string(&self.cfg),
            self.font_loaded.clone().unwrap_or_default(),
            self.svg.is_some()
        );
        if key == self.key {
            return;
        }
        self.key = key;
        if let Some(font) = &self.font {
            self.pages = jobs::build_pages(&self.doc, &self.cfg, font, self.svg.as_ref());
        }
        self.page = self.page.min(self.pages.len().saturating_sub(1));
        self.sim = 1.0;
        self.playing = false;
    }

    /// Сохраняем настройки и документ на диск сразу при любом изменении.
    fn save_cfg_if_changed(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let mut wrote = false;
        let s = config::to_string(&self.cfg);
        if s != self.cfg_saved {
            match config::save(&self.cfg_path, &s) {
                Ok(()) => {
                    self.cfg_saved = s;
                    wrote = true;
                }
                Err(e) => self.save_error = Some(format!("Не удалось сохранить настройки ({}): {e}", self.cfg_path.display())),
            }
        }
        if let Some(path) = self.session_path.clone() {
            self.prefs.all_fonts = self.all_fonts;
            let st = serde_json::to_string_pretty(&json!({"doc": self.doc, "prefs": self.prefs})).unwrap_or_default();
            if st != self.session_saved {
                match config::save(&path, &st) {
                    Ok(()) => {
                        self.session_saved = st;
                        wrote = true;
                    }
                    Err(e) => self.save_error = Some(format!("Не удалось сохранить документ ({}): {e}", path.display())),
                }
            }
        }
        if wrote {
            self.save_error = None;
            self.saved_at = now;
        }
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Текст, таблицы, рисунки, фото", &["txt", "md", "csv", "tsv", "svg", "png", "jpg", "jpeg", "bmp", "gif", "webp"])
            .add_filter("Все файлы", &["*"])
            .pick_file()
        {
            self.open_path(&p);
            self.notify(ctx, format!("Открыт {}", file_name(&p)), false);
        }
    }

    fn open_path(&mut self, p: &Path) {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        match ext.as_str() {
            "svg" | "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" => {
                self.doc.mode = Mode::Image;
                self.doc.svg_path = p.display().to_string();
                self.doc.show_image = true;
                self.doc.svg_pos = None;
            }
            "csv" | "tsv" => match table::read_csv(p) {
                Ok(rows) => {
                    self.doc.mode = Mode::Table;
                    self.doc.show_table = true;
                    self.doc.table = normalize(rows);
                }
                Err(e) => self.build_error = Some(format!("{e:#}")),
            },
            _ => match std::fs::read(p) {
                Ok(raw) => {
                    self.doc.mode = Mode::Text;
                    self.doc.text = String::from_utf8_lossy(&raw).trim_start_matches('\u{feff}').replace('\r', "");
                }
                Err(e) => self.build_error = Some(e.to_string()),
            },
        }
        self.doc.job_name = stem;
        self.reset_view();
    }

    /// Запоминаем состояния документа для Ctrl+Z / Ctrl+Y. Ввод текста и перетаскивание
    /// склеиваются в один шаг (фиксация после паузы 0.6 с и отпускания мыши).
    fn track_history(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if self.doc != self.prev_doc {
            self.prev_doc = self.doc.clone();
            self.last_edit = now;
        }
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        if self.doc != self.committed && !pointer_down && now - self.last_edit > 0.6 {
            self.commit_history();
        } else if self.doc != self.committed {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
    }

    fn commit_history(&mut self) {
        if self.doc != self.committed {
            self.undo.push(std::mem::replace(&mut self.committed, self.doc.clone()));
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    fn undo_step(&mut self, ctx: &egui::Context) {
        self.commit_history();
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.doc, prev));
            self.committed = self.doc.clone();
            self.prev_doc = self.doc.clone();
            self.selected_shape = None;
            self.notify(ctx, "↶ Отменено", false);
        } else {
            self.notify(ctx, "Нечего отменять", false);
        }
    }

    fn redo_step(&mut self, ctx: &egui::Context) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.doc, next));
            self.committed = self.doc.clone();
            self.prev_doc = self.doc.clone();
            self.selected_shape = None;
            self.notify(ctx, "↷ Повторено", false);
        }
    }

    /// Сдвиг выбранной фигуры или активного элемента листа.
    fn nudge(&mut self, dx: f64, dy: f64) {
        self.commit_history();
        if self.doc.mode == Mode::Draw {
            if let Some(sh) = self.selected_shape.and_then(|i| self.doc.shapes.get_mut(i)) {
                sh.translate(dx, dy);
            }
            return;
        }
        let Some(elem) = jobs::Elem::of_mode(self.doc.mode) else { return };
        let b = self.pages.get(self.page).and_then(|p| p.boxes.iter().find(|(e, _)| *e == elem).map(|(_, b)| *b));
        match elem {
            jobs::Elem::Text => self.doc.text_pos = (self.doc.text_pos.0 + dx, self.doc.text_pos.1 + dy),
            jobs::Elem::Table => {
                if let Some(p) = self.doc.table_pos.or(b.map(|b| (b[0], b[1]))) {
                    self.doc.table_pos = Some((p.0 + dx, p.1 + dy));
                }
            }
            jobs::Elem::Image => {
                if let Some(p) = self.doc.svg_pos.or(b.map(|b| (b[0], b[1]))) {
                    self.doc.svg_pos = Some((p.0 + dx, p.1 + dy));
                }
            }
            jobs::Elem::Drawing => {}
        }
    }

    /// Горячие клавиши (любая раскладка: проверяем и логическую, и физическую клавишу).
    fn hotkeys(&mut self, ctx: &egui::Context) {
        use egui::Key;
        if ctx.input(|i| i.key_pressed(Key::F1)) {
            self.show_keys = !self.show_keys;
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.selected_shape = None;
            self.selected_elem = None;
            self.draw_pts.clear();
            self.confirm_clear = false;
            self.show_keys = false;
        }
        // egui превращает Ctrl+C / Ctrl+V в события Copy / Paste
        let (copy, paste) = ctx.input(|i| {
            (
                i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))),
            )
        });
        if ctx.wants_keyboard_input() {
            return; // курсор в поле ввода — клавиши принадлежат полю
        }
        let events = ctx.input(|i| i.events.clone());
        let pressed = |k: Key| {
            events.iter().find_map(|e| match e {
                egui::Event::Key { key, physical_key, pressed: true, modifiers, .. }
                    if *key == k || *physical_key == Some(k) =>
                {
                    Some(*modifiers)
                }
                _ => None,
            })
        };
        let ctrl = |k: Key| pressed(k).filter(|m| m.command && !m.shift);
        let ctrl_shift = |k: Key| pressed(k).filter(|m| m.command && m.shift);
        let plain = |k: Key| pressed(k).filter(|m| !m.command && !m.alt);

        if ctrl(Key::Z).is_some() {
            self.undo_step(ctx);
        }
        if ctrl_shift(Key::Z).is_some() || ctrl(Key::Y).is_some() {
            self.redo_step(ctx);
        }
        if plain(Key::Delete).is_some() || plain(Key::Backspace).is_some() {
            self.delete_selected(ctx);
        }
        if ctrl(Key::O).is_some() {
            self.open_dialog(ctx);
        }
        if ctrl(Key::S).is_some() {
            self.save_dialog(ctx);
        }
        if ctrl(Key::P).is_some() && !self.pages.is_empty() && self.job_rx.is_none() {
            self.print(ctx);
        }
        if ctrl(Key::N).is_some() {
            self.confirm_clear = true;
        }
        if ctrl(Key::Equals).is_some() || ctrl(Key::Plus).is_some() || ctrl_shift(Key::Equals).is_some() {
            self.zoom_by(1.25);
        }
        if ctrl(Key::Minus).is_some() {
            self.zoom_by(0.8);
        }
        if ctrl(Key::Num0).is_some() {
            self.reset_view();
        }
        for (k, m) in [
            (Key::Num1, Mode::Text),
            (Key::Num2, Mode::Table),
            (Key::Num3, Mode::Image),
            (Key::Num4, Mode::Draw),
            (Key::Num5, Mode::Calibrate),
        ] {
            if ctrl(k).is_some() {
                self.doc.mode = m;
            }
        }
        // инструменты рисования — одной клавишей (как в Photoshop)
        if self.doc.mode == Mode::Draw {
            use crate::jobs::Tool;
            for (k, t) in [
                (Key::V, Tool::Select),
                (Key::B, Tool::Pencil),
                (Key::P, Tool::Pencil),
                (Key::L, Tool::Line),
                (Key::U, Tool::Rect),
                (Key::R, Tool::Rect),
                (Key::E, Tool::Circle),
                (Key::O, Tool::Circle),
                (Key::G, Tool::Grid),
            ] {
                if plain(k).is_some() {
                    self.doc.tool = t;
                }
            }
            if ctrl(Key::D).is_some() {
                self.commit_history();
                if let Some(sh) = self.selected_shape.and_then(|i| self.doc.shapes.get(i)).cloned() {
                    let mut c = sh;
                    c.translate(5.0, 5.0);
                    self.doc.shapes.push(c);
                    self.selected_shape = Some(self.doc.shapes.len() - 1);
                }
            }
            if copy {
                self.shape_clip = self.selected_shape.and_then(|i| self.doc.shapes.get(i)).cloned();
            }
            if paste {
                self.commit_history();
                if let Some(mut c) = self.shape_clip.clone() {
                    c.translate(5.0, 5.0);
                    self.shape_clip = Some(c.clone());
                    self.doc.shapes.push(c);
                    self.selected_shape = Some(self.doc.shapes.len() - 1);
                    self.doc.show_drawing = true;
                }
            }
        }
        // стрелки — сдвиг выбранного на 1 мм (Shift — 10 мм)
        for (k, d) in [(Key::ArrowLeft, (-1.0, 0.0)), (Key::ArrowRight, (1.0, 0.0)), (Key::ArrowUp, (0.0, -1.0)), (Key::ArrowDown, (0.0, 1.0))] {
            if let Some(m) = pressed(k).filter(|m| !m.command) {
                let step = if m.shift { 10.0 } else { 1.0 };
                self.nudge(d.0 * step, d.1 * step);
            }
        }
    }

    /// Delete: удалить выбранное (фигуру в «Рисовании» или текущий элемент листа).
    fn delete_selected(&mut self, ctx: &egui::Context) {
        self.commit_history();
        if let Some(i) = self.selected_shape.take() {
            if i < self.doc.shapes.len() {
                self.doc.shapes.remove(i);
                self.notify(ctx, "Фигура удалена (Ctrl+Z — вернуть)", false);
                return;
            }
        }
        let outlined = || -> Option<jobs::Elem> {
            let active = self.moving.as_ref().map(|m| m.elem).or(jobs::Elem::of_mode(self.doc.mode))?;
            self.pages.get(self.page)?.boxes.iter().any(|(e, _)| *e == active).then_some(active)
        };
        let target = self.selected_elem.take().or_else(outlined);
        match target {
            Some(jobs::Elem::Text) => {
                self.doc.text.clear();
                self.notify(ctx, "Текст удалён (Ctrl+Z — вернуть)", false);
            }
            Some(jobs::Elem::Table) => {
                self.doc.table = vec![vec![String::new(); 3]; 3];
                self.doc.show_table = false;
                self.notify(ctx, "Таблица удалена (Ctrl+Z — вернуть)", false);
            }
            Some(jobs::Elem::Image) => {
                self.doc.svg_path.clear();
                self.notify(ctx, "Рисунок удалён (Ctrl+Z — вернуть)", false);
            }
            Some(jobs::Elem::Drawing) => {
                self.doc.shapes.clear();
                self.notify(ctx, "Нарисованное удалено (Ctrl+Z — вернуть)", false);
            }
            None => self.notify(ctx, "Щёлкните по элементу на листе, чтобы выбрать его, затем Delete", false),
        }
    }

    /// Очистить весь лист: текст, таблица, рисунок, нарисованное.
    fn clear_all(&mut self) {
        self.commit_history();
        self.doc.text.clear();
        self.doc.table = vec![vec![String::new(); 3]; 3];
        self.doc.show_table = false;
        self.doc.svg_path.clear();
        self.doc.shapes.clear();
        self.doc.text_pos = (0.0, 0.0);
        self.doc.table_pos = None;
        self.doc.svg_pos = None;
        self.selected_shape = None;
    }

    /// Масштаб относительно центра превью.
    fn zoom_by(&mut self, k: f32) {
        let new = (self.zoom * k).clamp(0.5, 30.0);
        self.pan *= new / self.zoom;
        self.zoom = new;
    }

    /// Текстура пластины PEI (тёмно-золотистая «шагрень»), создаётся один раз.
    fn plate_texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        if let Some(t) = &self.plate_tex {
            return t.id();
        }
        let n = 512usize;
        let mut px = Vec::with_capacity(n * n);
        let mut seed: u32 = 0x9E37_79B9;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed & 0xFFFF) as f32 / 65535.0
        };
        for y in 0..n {
            for x in 0..n {
                let (fx, fy) = (x as f32 / n as f32 - 0.5, y as f32 / n as f32 - 0.5);
                let vignette = 1.0 - (fx * fx + fy * fy) * 0.5;
                let grain = rnd();
                let sparkle = if grain > 0.985 { 26.0 } else { 0.0 };
                let v = (grain * 10.0 + sparkle) * vignette;
                let (r, g, b) = (36.0 + v * 1.1, 33.0 + v, 28.0 + v * 0.7);
                px.push(Color32::from_rgb(r as u8, g as u8, b as u8));
            }
        }
        let img = egui::ColorImage { size: [n, n], pixels: px };
        let t = ctx.load_texture("pei_plate", img, egui::TextureOptions::LINEAR);
        let id = t.id();
        self.plate_tex = Some(t);
        id
    }

    fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    fn job_title(&self) -> String {
        let n = self.doc.job_name.trim();
        let n = if n.is_empty() { "plot" } else { n };
        if self.pages.len() > 1 {
            format!("{n} — стр. {}", self.page + 1)
        } else {
            n.to_string()
        }
    }

    fn export_to(&self, path: &Path) -> anyhow::Result<()> {
        let page = self.pages.get(self.page).ok_or_else(|| anyhow::anyhow!("Нечего печатать — документ пуст"))?;
        jobs::export(page, &self.cfg, path, &self.job_title())
    }

    fn save_dialog(&mut self, ctx: &egui::Context) {
        let name = format!("{}.gcode.3mf", safe_file_name(&self.job_title()));
        if let Some(p) = rfd::FileDialog::new().add_filter("Bambu задание", &["3mf"]).set_file_name(&name).save_file() {
            match self.export_to(&p) {
                Ok(()) => self.notify(ctx, format!("Сохранено: {}", p.display()), false),
                Err(e) => self.notify(ctx, format!("{e:#}"), true),
            }
        }
    }

    fn print(&mut self, ctx: &egui::Context) {
        let dir = std::env::temp_dir().join("bambu-plotter");
        let _ = std::fs::create_dir_all(&dir);
        let title = self.job_title();
        let ascii = translit(&title);
        let path = dir.join(format!("{ascii}.gcode.3mf"));
        if let Err(e) = self.export_to(&path) {
            self.notify(ctx, format!("{e:#}"), true);
            return;
        }
        if self.cfg.printer.method == "lan" {
            let (tx, rx) = channel();
            let pcfg = self.cfg.printer.clone();
            let ctx2 = ctx.clone();
            let remote = format!(
                "plot_{}.gcode.3mf",
                title.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>()
            );
            std::thread::spawn(move || {
                let r = (|| -> anyhow::Result<()> {
                    let data = std::fs::read(&path)?;
                    let p = printer::Printer::new(&pcfg)?;
                    p.upload(&remote, &data)?;
                    p.start_print(&remote)
                })();
                let _ = tx.send(r.map(|_| "Задание отправлено на принтер".to_string()).map_err(|e| format!("{e:#}")));
                ctx2.request_repaint();
            });
            self.job_rx = Some(rx);
            self.notify(ctx, "Отправка на принтер по сети…", false);
        } else {
            match connect::open(&path, &ascii) {
                Ok(()) => self.notify(ctx, "Открыто в Bambu Connect — выберите принтер и нажмите «Печать»", false),
                Err(e) => self.notify(ctx, format!("{e:#}"), true),
            }
        }
    }

    // ---------------- интерфейс ----------------

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .exact_height(60.0)
            .frame(egui::Frame::default().fill(PANEL).inner_margin(egui::Margin::symmetric(16, 0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    logo(ui);
                    ui.add_space(24.0);
                    for (m, label) in [
                        (Mode::Text, "✏  Текст"),
                        (Mode::Table, "📋  Таблица"),
                        (Mode::Image, "🖼  Рисунок"),
                        (Mode::Draw, "✒  Рисование"),
                        (Mode::Calibrate, "🎯  Калибровка"),
                    ] {
                        if tab(ui, self.doc.mode == m, label).clicked() {
                            self.doc.mode = m;
                            self.reset_view();
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let busy = self.job_rx.is_some();
                        let can = !self.pages.is_empty() && !busy;
                        let label = if busy { "⏳  Отправка…" } else { "▶  Печать" };
                        let btn = egui::Button::new(RichText::new(label).size(17.0).strong().color(Color32::WHITE))
                            .fill(if can { ACCENT } else { ACCENT_DIM })
                            .corner_radius(10)
                            .min_size(Vec2::new(150.0, 40.0));
                        let r = ui.add_enabled(can, btn);
                        crate::auto::track(&r, "Печать", "button");
                        let hint = if self.cfg.printer.method == "lan" {
                            "Отправить на принтер по локальной сети"
                        } else {
                            "Открыть в Bambu Connect и напечатать"
                        };
                        if r.on_hover_text(hint).clicked() {
                            self.print(ctx);
                        }
                        ui.add_space(6.0);
                        if tg(ui, "💾  Сохранить")
                            .on_hover_text("Сохранить .gcode.3mf (для microSD)")
                            .clicked()
                        {
                            self.save_dialog(ctx);
                        }
                        if tg(ui, "📂  Открыть").on_hover_text("Открыть текст, таблицу, SVG или фото (JPG, PNG…)").clicked() {
                            self.open_dialog(ctx);
                        }
                        let r = ui.add(ghost_button("🗑  Очистить"));
                        auto::track(&r, "Очистить всё", "button");
                        if r.on_hover_text("Очистить всё (Ctrl+N): убрать текст, таблицу, рисунок и нарисованное").clicked() {
                            self.confirm_clear = true;
                        }
                        let r = ui.add(ghost_button("⚙"));
                        auto::track(&r, if self.prefs.show_settings { "Скрыть настройки" } else { "Настройки" }, "button");
                        let tip = if self.prefs.show_settings { "Скрыть панель настроек" } else { "Показать панель настроек" };
                        if r.on_hover_text(tip).clicked() {
                            self.prefs.show_settings = !self.prefs.show_settings;
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(30.0)
            .frame(egui::Frame::default().fill(PANEL).inner_margin(egui::Margin::symmetric(14, 0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let now = ui.input(|i| i.time);
                    if let Some(e) = &self.save_error {
                        ui.label(RichText::new(format!("⚠ {e}")).color(DANGER));
                    } else if self.session_path.is_some() || !self.launch.is_test() {
                        let fresh = now - self.saved_at < 1.5;
                        let r = ui.label(RichText::new("✔ Сохранено").color(if fresh { ACCENT } else { DIM }));
                        r.on_hover_text(format!("Настройки и документ сохраняются автоматически:\n{}", self.cfg_path.display()));
                        if fresh {
                            ctx.request_repaint_after(std::time::Duration::from_millis(300));
                        }
                        ui.label(RichText::new("·").color(DIM));
                    }
                    if let Some(t) = &self.toast {
                        if now < t.until {
                            let c = if t.error { DANGER } else { ACCENT };
                            ui.label(RichText::new(if t.error { "●" } else { "●" }).color(c));
                            ui.label(RichText::new(&t.text).color(if t.error { DANGER } else { Color32::WHITE }));
                            ctx.request_repaint_after(std::time::Duration::from_millis(500));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.cfg.printer.method == "lan" {
                            ui.label(RichText::new(format!("LAN: {}", self.cfg.printer.ip)).color(DIM));
                        } else if self.connect_ok {
                            ui.label(RichText::new("Bambu Connect ✔").color(DIM));
                        } else {
                            ui.hyperlink_to(
                                RichText::new("⚠ Установите Bambu Connect").color(Color32::from_rgb(240, 180, 60)),
                                connect::DOWNLOAD_URL,
                            );
                        }
                        ui.label(RichText::new("·").color(DIM));
                        ui.label(RichText::new("Перетащите файл в окно, чтобы открыть").color(DIM));
                        ui.label(RichText::new("·").color(DIM));
                        if tsb(ui, "⌨ Клавиши (F1)").clicked() {
                            self.show_keys = true;
                        }
                    });
                });
            });
    }

    fn editor_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("editor")
            .resizable(false)
            .exact_width(390.0)
            .frame(egui::Frame::default().fill(PANEL).inner_margin(egui::Margin::same(14)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    card(ui, "НАЗВАНИЕ ЗАДАНИЯ", |ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.doc.job_name)
                                .desired_width(f32::INFINITY)
                                .font(FontId::proportional(16.0)),
                        );
                        crate::auto::track(&r, "Название задания", "text");
                    });
                    match self.doc.mode {
                        Mode::Text => self.text_editor(ui),
                        Mode::Table => self.table_editor(ui),
                        Mode::Image => self.image_editor(ui),
                        Mode::Draw => self.draw_editor(ui),
                        Mode::Calibrate => self.calibrate_editor(ui),
                    }
                });
            });
    }

    /// Блок «На листе»: показывать ли элемент и сброс его положения.
    fn sheet_card(&mut self, ui: &mut Ui, elem: jobs::Elem) {
        let d = &mut self.doc;
        card(ui, "НА ЛИСТЕ", |ui| {
            ui.horizontal(|ui| {
                let (flag, label) = match elem {
                    jobs::Elem::Text => (&mut d.show_text, "Печатать текст"),
                    jobs::Elem::Table => (&mut d.show_table, "Печатать таблицу"),
                    jobs::Elem::Image => (&mut d.show_image, "Печатать рисунок"),
                    jobs::Elem::Drawing => (&mut d.show_drawing, "Печатать нарисованное"),
                };
                tcb(ui, flag, label);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if elem != jobs::Elem::Drawing && tsb(ui, "↺ На место").on_hover_text("Вернуть элемент в положение по умолчанию").clicked() {
                        match elem {
                            jobs::Elem::Text => d.text_pos = (0.0, 0.0),
                            jobs::Elem::Table => d.table_pos = None,
                            jobs::Elem::Image => d.svg_pos = None,
                            jobs::Elem::Drawing => {}
                        }
                    }
                });
            });
            ui.label(
                RichText::new("Всё печатается на одном листе: текст, таблица, рисунок и нарисованное. Элементы можно перетаскивать мышью на превью.")
                    .color(DIM)
                    .small(),
            );
        });
    }

    fn text_editor(&mut self, ui: &mut Ui) {
        self.sheet_card(ui, jobs::Elem::Text);
        let before = self.doc.text.clone();
        card(ui, "ТЕКСТ", |ui| {
            let r = ui.add(
                egui::TextEdit::multiline(&mut self.doc.text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(16)
                    .font(FontId::proportional(15.0))
                    .hint_text("Введите текст…"),
            );
            crate::auto::track(&r, "Поле текста", "text");
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} симв.", self.doc.text.chars().count())).color(DIM).small());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if tsb(ui, "Очистить").clicked() {
                        self.doc.text.clear();
                    }
                    if tsb(ui, "📋 Вставить").clicked() {
                        if let Ok(t) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                            self.doc.text.push_str(&t.replace('\r', ""));
                        }
                    }
                });
            });
        });
        card(ui, "ВЫРАВНИВАНИЕ", |ui| {
            ui.horizontal(|ui| {
                for (v, l) in [(0u8, "⬅ Влево"), (1, "↔ По центру"), (2, "➡ Вправо")] {
                    if segment(ui, self.doc.align == v, l).clicked() {
                        self.doc.align = v;
                    }
                }
            });
        });
        if self.doc.text != before {
            self.doc.show_text = true;
        }
        self.font_card(ui);
    }

    fn table_editor(&mut self, ui: &mut Ui) {
        self.sheet_card(ui, jobs::Elem::Table);
        let before = self.doc.table.clone();
        card(ui, "ТАБЛИЦА", |ui| {
            ui.horizontal_wrapped(|ui| {
                let cols = self.doc.table.iter().map(|r| r.len()).max().unwrap_or(0);
                if tb(ui, "+ Строка").clicked() {
                    self.doc.table.push(vec![String::new(); cols.max(1)]);
                }
                if tb(ui, "+ Столбец").clicked() {
                    for r in &mut self.doc.table {
                        r.push(String::new());
                    }
                    if self.doc.table.is_empty() {
                        self.doc.table.push(vec![String::new()]);
                    }
                }
                if tb(ui, "− Строка").clicked() {
                    self.doc.table.pop();
                }
                if tb(ui, "− Столбец").clicked() {
                    for r in &mut self.doc.table {
                        r.pop();
                    }
                }
            });
            ui.horizontal(|ui| {
                if tb(ui, "📋 Вставить из Excel").on_hover_text("Скопируйте ячейки в Excel и нажмите").clicked() {
                    if let Ok(t) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                        if let Ok(rows) = table::parse(&t) {
                            self.doc.table = normalize(rows);
                        }
                    }
                }
                if tb(ui, "🗑 Очистить").clicked() {
                    self.doc.table = vec![vec![String::new(); 3]; 3];
                }
            });
            ui.add_space(6.0);
            let cols = self.doc.table.iter().map(|r| r.len()).max().unwrap_or(0);
            for r in &mut self.doc.table {
                r.resize(cols, String::new());
            }
            // ширина столбца — по самому длинному тексту
            let col_w: Vec<f32> = (0..cols)
                .map(|c| {
                    let w = self
                        .doc
                        .table
                        .iter()
                        .map(|r| {
                            ui.fonts(|f| f.layout_no_wrap(r[c].clone(), FontId::proportional(15.0), Color32::WHITE).size().x)
                        })
                        .fold(0.0, f32::max);
                    (w + 26.0).clamp(56.0, 240.0)
                })
                .collect();
            egui::ScrollArea::both().max_height(360.0).id_salt("tbl_scroll").show(ui, |ui| {
                egui::Grid::new("tbl").spacing(Vec2::new(4.0, 4.0)).show(ui, |ui| {
                    for (ri, row) in self.doc.table.iter_mut().enumerate() {
                        for (ci, cell) in row.iter_mut().enumerate() {
                            let mut te = egui::TextEdit::singleline(cell).desired_width(col_w[ci]);
                            if ri == 0 && self.doc.table_header {
                                te = te.font(FontId::proportional(15.0)).text_color(ACCENT);
                            }
                            let r = ui.add_sized([col_w[ci], 26.0], te);
                            crate::auto::track(&r, &format!("ячейка {}:{}", ri + 1, ci + 1), "text");
                        }
                        ui.end_row();
                    }
                });
            });
            ui.add_space(4.0);
            tcb(ui, &mut self.doc.table_header, "Первая строка — заголовок (повторяется на каждой странице)");
        });
        if self.doc.table != before {
            self.doc.show_table = true;
        }
        self.font_card(ui);
    }

    fn image_editor(&mut self, ui: &mut Ui) {
        self.sheet_card(ui, jobs::Elem::Image);
        card(ui, "РИСУНОК: SVG ИЛИ ФОТО", |ui| {
            let r = ui.add(
                egui::Button::new(RichText::new("📂  Выбрать файл (SVG, JPG, PNG…)").size(15.0)).min_size(Vec2::new(ui.available_width(), 38.0)),
            );
            crate::auto::track(&r, "Выбрать SVG-файл", "button");
            if r.clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Рисунки и фото", &["svg", "png", "jpg", "jpeg", "bmp", "gif", "webp"])
                    .pick_file()
                {
                    self.doc.svg_path = p.display().to_string();
                    self.doc.show_image = true;
                    self.doc.svg_pos = None;
                    self.doc.job_name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                }
            }
            if self.doc.svg_path.is_empty() {
                ui.label(RichText::new("Файл не выбран. SVG рисуется как есть; фото и картинки (JPG, PNG, BMP, GIF, WebP) превращаются в линии.").color(DIM));
            } else {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(file_name(Path::new(&self.doc.svg_path))).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if tsb(ui, "🗑 Убрать рисунок").clicked() {
                            self.doc.svg_path.clear();
                        }
                    });
                });
            }
        });
        if let Some(jobs::ImageSrc::Raster(r)) = &self.svg {
            // превью исходной картинки
            if self.img_tex.is_none() {
                let (w, h, px) = &r.thumb;
                let img = egui::ColorImage::from_rgba_unmultiplied([*w, *h], px);
                self.img_tex = Some(ui.ctx().load_texture("img_thumb", img, egui::TextureOptions::LINEAR));
            }
            let p = &mut self.doc.img;
            let tex = self.img_tex.clone();
            card(ui, "ФОТО → ЛИНИИ", |ui| {
                if let Some(t) = &tex {
                    let sz = t.size_vec2();
                    let k = (ui.available_width() / sz.x).min(160.0 / sz.y).min(1.0);
                    ui.image((t.id(), sz * k));
                }
                ui.label(RichText::new("Стиль").color(DIM).small());
                ui.horizontal(|ui| {
                    use crate::raster::Style;
                    for (st, l, tip) in [
                        (Style::Hatch, "Штриховка", "Слои штрихов: чем темнее, тем гуще. Для фото."),
                        (Style::Waves, "Волны", "Строки-синусоиды, почти одной линией. Для портретов."),
                        (Style::Contour, "Контур", "Обводка границ. Для логотипов, рисунков, надписей."),
                    ] {
                        if segment(ui, p.style == st, l).on_hover_text(tip).clicked() {
                            p.style = st;
                        }
                    }
                });
                ui.add(egui::Slider::new(&mut p.spacing, 0.3..=4.0).text("шаг линий, мм").fixed_decimals(1));
                ui.add(egui::Slider::new(&mut p.brightness, -1.0..=1.0).text("яркость").fixed_decimals(2));
                ui.add(egui::Slider::new(&mut p.contrast, 0.3..=3.0).text("контраст").fixed_decimals(2));
                tcb(ui, &mut p.invert, "Инвертировать (светлое ↔ тёмное)");
                ui.label(RichText::new("Меньше шаг — больше деталей, но дольше печать. Ручка обычно 0.4–0.7 мм.").color(DIM).small());
            });
        }
        card(ui, "РАЗМЕЩЕНИЕ", |ui| {
            tcb(ui, &mut self.doc.svg_fit, "Растянуть на весь лист");
            ui.add_enabled(
                !self.doc.svg_fit,
                egui::Slider::new(&mut self.doc.svg_scale, 5.0..=500.0).text("масштаб").suffix(" %").logarithmic(true),
            );
            ui.horizontal(|ui| {
                if tb(ui, "⟲ Повернуть влево").clicked() {
                    self.doc.svg_rot = (self.doc.svg_rot + 3) % 4;
                }
                if tb(ui, "⟳ Повернуть вправо").clicked() {
                    self.doc.svg_rot = (self.doc.svg_rot + 1) % 4;
                }
            });
        });
    }

    fn draw_editor(&mut self, ui: &mut Ui) {
        use crate::jobs::Tool;
        self.sheet_card(ui, jobs::Elem::Drawing);
        card(ui, "ИНСТРУМЕНТ", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (t, l) in [
                    (Tool::Select, "Выбор"),
                    (Tool::Pencil, "✏ Карандаш"),
                    (Tool::Line, "Линия"),
                    (Tool::Rect, "Прямоугольник"),
                    (Tool::Circle, "Круг"),
                    (Tool::Grid, "Сетка"),
                ] {
                    if segment(ui, self.doc.tool == t, l).clicked() {
                        self.doc.tool = t;
                    }
                }
            });
            ui.label(
                RichText::new("Рисуйте левой кнопкой мыши прямо на листе. «Выбор»: щелчок — выделить, перетащить — сдвинуть, Delete — удалить, Ctrl+Z — отменить. Сдвиг вида — правой кнопкой, масштаб — колесом.")
                    .color(DIM)
                    .small(),
            );
        });
        card(ui, "ПРИВЯЗКА", |ui| {
            ui.horizontal(|ui| {
                for (v, l) in [(0.0, "Нет"), (1.0, "1 мм"), (5.0, "5 мм"), (10.0, "10 мм")] {
                    if segment(ui, (self.doc.snap - v).abs() < 1e-9, l).clicked() {
                        self.doc.snap = v;
                    }
                }
            });
        });
        card(ui, "СЕТКА", |ui| {
            ui.add(egui::Slider::new(&mut self.doc.grid_step, 1.0..=50.0).text("клетка, мм").fixed_decimals(0));
            tcb(ui, &mut self.doc.grid_labels, "Подписи в мм");
            ui.add_space(4.0);
            let r = ui.add(
                egui::Button::new(RichText::new("📐  Тест масштаба: сетка 100×100 мм").size(15.0))
                    .min_size(Vec2::new(ui.available_width(), 36.0)),
            );
            auto::track(&r, "Тест масштаба", "button");
            if r.on_hover_text("Нарисуйте и измерьте линейкой: квадрат должен быть ровно 100 × 100 мм").clicked() {
                self.doc.shapes.extend(jobs::scale_test());
                self.doc.show_drawing = true;
            }
        });
        card(ui, "ФИГУРЫ", |ui| {
            ui.label(format!("Фигур на листе: {}", self.doc.shapes.len()));
            ui.horizontal(|ui| {
                if tb(ui, "⟲ Отменить").clicked() {
                    self.doc.shapes.pop();
                }
                if tb(ui, "🗑 Очистить лист").clicked() {
                    self.doc.shapes.clear();
                }
            });
        });
    }

    fn calibrate_editor(&mut self, ui: &mut Ui) {
        card(ui, "КАЛИБРОВКА ВЫСОТЫ РУЧКИ", |ui| {
            ui.label("Принтер нарисует линии на разной высоте Z — сверху самая высокая.");
            ui.label("Найдите верхнюю линию с ровным чётким следом и введите её Z ниже.");
            ui.add_space(6.0);
            ui.add(egui::Slider::new(&mut self.doc.cal_step, 0.05..=1.0).text("шаг, мм").fixed_decimals(2));
            ui.add(egui::Slider::new(&mut self.doc.cal_steps, 1..=8).text("шагов вверх/вниз"));
        });
        card(ui, "РЕЗУЛЬТАТ", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Z пишущей ручки").size(16.0));
                ui.add(egui::DragValue::new(&mut self.cfg.pen.z_down).speed(0.05).range(0.0..=100.0).suffix(" мм").fixed_decimals(2));
            });
            ui.add_space(4.0);
            let levels = jobs::calibration_levels(&self.doc, &self.cfg);
            egui::Grid::new("levels").striped(true).show(ui, |ui| {
                for (i, z) in levels.iter().enumerate() {
                    ui.label(RichText::new(format!("{}", i + 1)).color(DIM));
                    ui.label(format!("Z = {z:.2}"));
                    if tsb(ui, "выбрать").clicked() {
                        self.cfg.pen.z_down = *z;
                    }
                    ui.end_row();
                }
            });
        });
        card(ui, "КОРРЕКЦИЯ МАСШТАБА", |ui| {
            ui.label(RichText::new("1. Напечатайте «Тест масштаба» (ниже). 2. Измерьте линейкой сторону квадрата — должно быть 100 мм. 3. Введите, что получилось, и нажмите «Применить».").color(DIM).small());
            row(ui, "По горизонтали (X), мм", |ui| ui.add(egui::DragValue::new(&mut self.meas_x).range(50.0..=150.0).speed(0.1).fixed_decimals(1)));
            row(ui, "По вертикали (Y), мм", |ui| ui.add(egui::DragValue::new(&mut self.meas_y).range(50.0..=150.0).speed(0.1).fixed_decimals(1)));
            ui.horizontal(|ui| {
                if tb(ui, "✔ Применить коррекцию").clicked() {
                    self.cfg.pen.scale_x *= 100.0 / self.meas_x;
                    self.cfg.pen.scale_y *= 100.0 / self.meas_y;
                    self.meas_x = 100.0;
                    self.meas_y = 100.0;
                }
                if tsb(ui, "Сбросить").clicked() {
                    self.cfg.pen.scale_x = 1.0;
                    self.cfg.pen.scale_y = 1.0;
                }
            });
            ui.label(format!(
                "Сейчас: X {:.2}%, Y {:.2}%",
                self.cfg.pen.scale_x * 100.0,
                self.cfg.pen.scale_y * 100.0
            ));
        });
        card(ui, "ПРОВЕРОЧНЫЕ ЛИСТЫ", |ui| {
            ui.label(RichText::new("Направление: стрелка к задней стенке и буквы R F У — видно, не отражён ли рисунок.").color(DIM).small());
            if tb(ui, "⬆ Открыть тест направления").clicked() {
                self.doc.mode = Mode::Draw;
                self.doc.shapes = jobs::orientation_test(self.cfg.usable_area());
            }
            ui.label(RichText::new("Масштаб: квадрат 100×100 мм с клеткой 10 мм — измерьте линейкой.").color(DIM).small());
            if tb(ui, "📐 Открыть тест масштаба").clicked() {
                self.doc.mode = Mode::Draw;
                self.doc.shapes = jobs::scale_test();
            }
        });
    }

    fn font_card(&mut self, ui: &mut Ui) {
        if let Some(rx) = &self.font_rx {
            if let Ok(list) = rx.try_recv() {
                self.font_list = list;
                self.font_rx = None;
            }
        }
        card(ui, "ШРИФТ", |ui| {
            let current = if self.cfg.text.font.is_empty() {
                format!("✒ {}", Font::default_name())
            } else {
                self.font_list
                    .iter()
                    .find(|f| f.path == self.cfg.text.font)
                    .map(|f| f.name.clone())
                    .unwrap_or_else(|| file_name(Path::new(&self.cfg.text.font)))
            };
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.font_filter).hint_text("🔍 Поиск шрифта").desired_width(ui.available_width() - 60.0));
                if tb(ui, "📂").on_hover_text("Выбрать файл шрифта (.ttf / .otf)").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Шрифты", &["ttf", "otf"]).pick_file() {
                        self.cfg.text.font = p.display().to_string();
                    }
                }
            });
            let filter = self.font_filter.to_lowercase();
            let combo = egui::ComboBox::from_id_salt("font")
                .width(ui.available_width())
                .height(420.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if tsl(ui, self.cfg.text.font.is_empty(), "✒ Однолинейный (встроенный) — для ручки").clicked() {
                        self.cfg.text.font.clear();
                    }
                    if self.font_rx.is_some() {
                        ui.label(RichText::new("Загрузка списка шрифтов…").color(DIM));
                    }
                    for f in &self.font_list {
                        if (!self.all_fonts && !f.cyrillic) || (!filter.is_empty() && !f.name.to_lowercase().contains(&filter)) {
                            continue;
                        }
                        if tsl(ui, self.cfg.text.font == f.path, &f.name).clicked() {
                            self.cfg.text.font = f.path.clone();
                        }
                    }
                });
            crate::auto::track(&combo.response, "Выбор шрифта", "combo");
            tcb(ui, &mut self.all_fonts, "Показывать шрифты без кириллицы");
            ui.add_space(4.0);
            ui.add(egui::Slider::new(&mut self.cfg.text.size, 2.0..=40.0).text("высота букв, мм").fixed_decimals(1));
            ui.add(egui::Slider::new(&mut self.cfg.text.line_spacing, 0.8..=3.0).text("интервал").fixed_decimals(2));
            ui.label(RichText::new("Встроенный однолинейный шрифт рисует каждую букву одной линией. Шрифты Windows рисуются контуром (двумя линиями).").color(DIM).small());
        });
    }

    fn settings_panel(&mut self, ctx: &egui::Context) {
        if !self.prefs.show_settings {
            return;
        }
        egui::SidePanel::right("settings")
            .resizable(false)
            .exact_width(310.0)
            .frame(egui::Frame::default().fill(PANEL).inner_margin(egui::Margin::same(14)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    self.paper_card(ui);
                    self.pen_card(ui);
                    self.speed_card(ui);
                    self.printer_card(ui);
                    self.view_card(ui);
                });
            });
    }

    fn paper_card(&mut self, ui: &mut Ui) {
        let auto_origin = self.cfg.paper_origin();
        let p = &mut self.cfg.paper;
        card(ui, "📄 БУМАГА", |ui| {
            let preset = PAPER_PRESETS
                .iter()
                .find(|(_, w, h)| (*w - p.width).abs() < 0.01 && (*h - p.height).abs() < 0.01)
                .map(|(n, _, _)| n.to_string())
                .unwrap_or_else(|| "Свой размер".into());
            let combo = egui::ComboBox::from_id_salt("paper").width(ui.available_width()).height(600.0).selected_text(preset).show_ui(ui, |ui| {
                for (n, w, h) in PAPER_PRESETS {
                    if tsl(ui, false, format!("{n}  ({w}×{h})")).clicked() {
                        p.width = *w;
                        p.height = *h;
                    }
                }
            });
            crate::auto::track(&combo.response, "Формат бумаги", "combo");
            row(ui, "Ширина", |ui| ui.add(egui::DragValue::new(&mut p.width).range(20.0..=420.0).speed(1.0).suffix(" мм")));
            row(ui, "Высота", |ui| ui.add(egui::DragValue::new(&mut p.height).range(20.0..=420.0).speed(1.0).suffix(" мм")));
            row(ui, "Поля", |ui| ui.add(egui::DragValue::new(&mut p.margin).range(0.0..=60.0).speed(0.5).suffix(" мм")));
            let mut centered = p.origin_x.is_none();
            if tcb(ui, &mut centered, "Лист по центру (в зоне ручки)").changed() {
                if centered {
                    p.origin_x = None;
                    p.origin_y = None;
                } else {
                    let (x, y) = auto_origin;
                    p.origin_x = Some(x);
                    p.origin_y = Some(y);
                }
            }
            if let (Some(x), Some(y)) = (p.origin_x.as_mut(), p.origin_y.as_mut()) {
                row(ui, "Левый край X", |ui| ui.add(egui::DragValue::new(x).range(-200.0..=BED_W).speed(0.5).suffix(" мм")));
                row(ui, "Нижний край Y", |ui| ui.add(egui::DragValue::new(y).range(-200.0..=BED_H).speed(0.5).suffix(" мм")));
            }
        });
    }

    fn pen_card(&mut self, ui: &mut Ui) {
        let pen = &mut self.cfg.pen;
        card(ui, "🖊 РУЧКА", |ui| {
            row(ui, "Z — пишет", |ui| {
                ui.add(egui::DragValue::new(&mut pen.z_down).range(0.0..=100.0).speed(0.05).fixed_decimals(2).suffix(" мм"))
            });
            row(ui, "Z — поднята", |ui| {
                ui.add(egui::DragValue::new(&mut pen.z_up).range(0.0..=200.0).speed(0.1).fixed_decimals(1).suffix(" мм"))
            });
            ui.label(RichText::new("Смещение кончика ручки от сопла").color(DIM).small());
            row(ui, "X (+ вправо)", |ui| {
                ui.add(egui::DragValue::new(&mut pen.offset_x).range(-100.0..=100.0).speed(0.1).fixed_decimals(1).suffix(" мм"))
            });
            row(ui, "Y (+ назад)", |ui| {
                ui.add(egui::DragValue::new(&mut pen.offset_y).range(-100.0..=100.0).speed(0.1).fixed_decimals(1).suffix(" мм"))
            });
            row(ui, "Пауза после касания", |ui| {
                ui.add(egui::DragValue::new(&mut pen.pen_down_delay_ms).range(0..=2000).speed(5.0).suffix(" мс"))
            });
            tcb(ui, &mut pen.home, "Хоминг (G28) перед рисованием");
            ui.label(RichText::new("Если отпечаток зеркальный — см. «Калибровка → Тест направления»").color(DIM).small());
            let mut sx = pen.scale_x * 100.0;
            let mut sy = pen.scale_y * 100.0;
            row(ui, "Масштаб X", |ui| ui.add(egui::DragValue::new(&mut sx).range(50.0..=150.0).speed(0.05).fixed_decimals(2).suffix(" %")));
            row(ui, "Масштаб Y", |ui| ui.add(egui::DragValue::new(&mut sy).range(50.0..=150.0).speed(0.05).fixed_decimals(2).suffix(" %")));
            pen.scale_x = sx / 100.0;
            pen.scale_y = sy / 100.0;
            tcb(ui, &mut pen.invert_x, "Отразить по горизонтали (ось X)");
            tcb(ui, &mut pen.invert_y, "Отразить по вертикали (ось Y)");
        });
    }

    fn speed_card(&mut self, ui: &mut Ui) {
        let pen = &mut self.cfg.pen;
        card(ui, "⚡ СКОРОСТЬ", |ui| {
            ui.spacing_mut().slider_width = ui.available_width() - 90.0;
            for (label, v, range) in [
                ("Рисование", &mut pen.draw_speed, 5.0..=200.0),
                ("Переезды", &mut pen.travel_speed, 20.0..=400.0),
                ("Подъём ручки", &mut pen.z_speed, 2.0..=30.0),
            ] {
                ui.label(RichText::new(label).color(DIM).small());
                ui.add(egui::Slider::new(v, range).suffix(" мм/с").fixed_decimals(0));
            }
        });
    }

    fn printer_card(&mut self, ui: &mut Ui) {
        let connect_ok = self.connect_ok;
        let pr = &mut self.cfg.printer;
        card(ui, "🖨 ПЕЧАТЬ", |ui| {
            ui.horizontal(|ui| {
                if segment(ui, pr.method != "lan", "Bambu Connect").clicked() {
                    pr.method = "connect".into();
                }
                if segment(ui, pr.method == "lan", "LAN").clicked() {
                    pr.method = "lan".into();
                }
            });
            if pr.method == "lan" {
                ui.label(RichText::new("Нужны LAN Only + Developer Mode на принтере (Handy при этом не работает).").color(DIM).small());
                row(ui, "IP", |ui| ui.add(egui::TextEdit::singleline(&mut pr.ip).desired_width(150.0)));
                row(ui, "Серийный №", |ui| ui.add(egui::TextEdit::singleline(&mut pr.serial).desired_width(150.0)));
                row(ui, "Код доступа", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut pr.access_code).password(true).desired_width(150.0))
                });
            } else if connect_ok {
                ui.label(RichText::new("Печать через облако, вход по email. Bambu Handy продолжает работать.").color(DIM).small());
            } else {
                ui.label(RichText::new("Bambu Connect не найден.").color(DANGER));
                ui.hyperlink_to("Скачать Bambu Connect", connect::DOWNLOAD_URL);
            }
        });
    }

    fn view_card(&mut self, ui: &mut Ui) {
        card(ui, "🎨 ВИД", |ui| {
            ui.horizontal(|ui| {
                ui.label("Цвет чернил в превью");
                ui.color_edit_button_srgb(&mut self.prefs.ink);
            });
            tcb(ui, &mut self.prefs.show_travel, "Показывать холостые переезды");
        });
    }

    fn preview(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().frame(egui::Frame::default().fill(BG)).show(ctx, |ui| {
            // панель инструментов над превью
            egui::Frame::default().inner_margin(egui::Margin::symmetric(14, 8)).show(ui, |ui| {
                let w = ui.available_width();
                ui.allocate_ui_with_layout(Vec2::new(w, 32.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_height(32.0);
                    let n = self.pages.len();
                    if n > 1 {
                        let r = ui.add_enabled(self.page > 0, egui::Button::new("◀"));
                        crate::auto::track(&r, "Предыдущая страница", "button");
                        if r.clicked() {
                            self.page -= 1;
                            self.sim = 1.0;
                        }
                        ui.label(RichText::new(format!("Страница {} из {}", self.page + 1, n)).strong());
                        let r = ui.add_enabled(self.page + 1 < n, egui::Button::new("▶"));
                        crate::auto::track(&r, "Следующая страница", "button");
                        if r.clicked() {
                            self.page += 1;
                            self.sim = 1.0;
                        }
                        ui.separator();
                    }
                    let play = if self.playing { "⏸" } else { "▶" };
                    let r = ui.add(egui::Button::new(RichText::new(play).size(16.0)).min_size(Vec2::new(34.0, 28.0)));
                    crate::auto::track(&r, "Симуляция", "button");
                    if r.on_hover_text("Симуляция рисования").clicked() {
                        if !self.playing && self.sim >= 1.0 {
                            self.sim = 0.0;
                        }
                        self.playing = !self.playing;
                    }
                    ui.spacing_mut().slider_width = (ui.available_width() - 380.0).clamp(50.0, 320.0);
                    if ui.add(egui::Slider::new(&mut self.sim, 0.0..=1.0).show_value(false)).changed() {
                        self.playing = false;
                    }
                    egui::ComboBox::from_id_salt("simspeed")
                        .width(70.0)
                        .selected_text(format!("{}×", self.prefs.sim_speed))
                        .show_ui(ui, |ui| {
                            for s in [1.0, 5.0, 10.0, 30.0, 100.0] {
                                ui.selectable_value(&mut self.prefs.sim_speed, s, format!("{s}×"));
                            }
                        });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if tb(ui, "⟲ Вписать").on_hover_text("Показать весь стол (или двойной клик)").clicked() {
                            self.reset_view();
                        }
                        if tb(ui, "+").on_hover_text("Увеличить").clicked() {
                            self.zoom_by(1.25);
                        }
                        ui.label(RichText::new(format!("{:.0}%", self.zoom * 100.0)).strong());
                        if tb(ui, "−").on_hover_text("Уменьшить").clicked() {
                            self.zoom_by(0.8);
                        }
                    });
                });
            });

            let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
            crate::auto::track(&resp, "Превью", "canvas");
            let rect = resp.rect;

            // зум / панорамирование (в режиме рисования левая кнопка рисует, двигаем правой/средней)
            let drawing_mode = self.doc.mode == Mode::Draw;
            let center_shift = Vec2::new(0.0, -26.0);
            let pan_drag = resp.dragged_by(egui::PointerButton::Secondary)
                || resp.dragged_by(egui::PointerButton::Middle)
                || (!drawing_mode && resp.dragged_by(egui::PointerButton::Primary));
            let pan_delta = if pan_drag { resp.drag_delta() } else { Vec2::ZERO };
            if resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    if let Some(m) = resp.hover_pos() {
                        let old = self.zoom;
                        let new = (old * (scroll * 0.0025).exp()).clamp(0.5, 30.0);
                        let c = m - (rect.center() + center_shift);
                        self.pan = (self.pan - c) * (new / old) + c;
                        self.zoom = new;
                    }
                }
            }
            if resp.double_clicked() && !drawing_mode {
                self.reset_view();
            }

            // вид по умолчанию — весь стол A1 с линейками
            let (ox, oy) = self.cfg.paper_origin();
            let (pw, ph) = (self.cfg.paper.width, self.cfg.paper.height);
            let base = (rect.width() / (BED_W as f32 + 60.0)).min((rect.height() - 60.0) / (BED_H as f32 + 60.0));
            let s = base * self.zoom;
            let (cx, cy) = (BED_W / 2.0, BED_H / 2.0 - 4.0);
            let center = rect.center() + self.pan + center_shift;
            let to = |x: f64, y: f64| Pos2::new(center.x + ((x - cx) as f32) * s, center.y - ((y - cy) as f32) * s);
            let mm = |v: f64| v as f32 * s;

            // координаты рабочей области листа (мм от левого верхнего угла, Y вниз) <-> экран
            let margin = self.cfg.paper.margin;
            let (ax0, atop) = (ox + margin, oy + ph - margin);
            let snap = self.doc.snap;
            let screen_to_area = |p: Pos2, use_snap: bool| -> (f64, f64) {
                let bx = cx + ((p.x - center.x) / s) as f64;
                let by = cy - ((p.y - center.y) / s) as f64;
                let (mut x, mut y) = (bx - ax0, atop - by);
                if use_snap && snap > 0.0 {
                    x = (x / snap).round() * snap;
                    y = (y / snap).round() * snap;
                }
                (x, y)
            };
            let area_to_screen = |(x, y): (f64, f64)| to(ax0 + x, atop - y);
            self.view_tf = Some((center, s, cx, cy, ax0, atop));

            // рисование мышью
            if drawing_mode && self.doc.tool == crate::jobs::Tool::Select {
                let pick = |p: Pos2, shapes: &[crate::jobs::Shape], font: Option<&Font>| -> Option<usize> {
                    let font = font?;
                    let a = screen_to_area(p, false);
                    let tol = (10.0 / s as f64).max(1.0);
                    shapes
                        .iter()
                        .enumerate()
                        .map(|(i, sh)| (i, sh.distance_to(a, font)))
                        .filter(|(_, d)| *d <= tol)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(i, _)| i)
                };
                if resp.clicked() {
                    self.selected_shape = resp.interact_pointer_pos().and_then(|p| pick(p, &self.doc.shapes, self.font.as_ref()));
                    self.selected_elem = None;
                }
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.selected_shape = pick(p, &self.doc.shapes, self.font.as_ref());
                        if self.selected_shape.is_some() {
                            self.commit_history();
                            self.shape_drag = Some((screen_to_area(p, false), (0.0, 0.0)));
                        }
                    }
                }
                if let (Some(i), Some((grab, applied)), Some(p)) = (self.selected_shape, self.shape_drag, resp.interact_pointer_pos()) {
                    let a = screen_to_area(p, false);
                    let d = ((a.0 - grab.0).round(), (a.1 - grab.1).round());
                    let inc = (d.0 - applied.0, d.1 - applied.1);
                    if inc != (0.0, 0.0) {
                        if let Some(sh) = self.doc.shapes.get_mut(i) {
                            sh.translate(inc.0, inc.1);
                        }
                        self.shape_drag = Some((grab, d));
                    }
                }
                if !resp.dragged() {
                    self.shape_drag = None;
                }
            }
            if drawing_mode && self.doc.tool != crate::jobs::Tool::Select && resp.clicked() {
                if let (Some(p), Some(font)) = (resp.interact_pointer_pos(), self.font.as_ref()) {
                    let a = screen_to_area(p, false);
                    let tol = (10.0 / s as f64).max(1.0);
                    self.selected_shape = self
                        .doc
                        .shapes
                        .iter()
                        .enumerate()
                        .map(|(i, sh)| (i, sh.distance_to(a, font)))
                        .filter(|(_, d)| *d <= tol)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(i, _)| i);
                    self.selected_elem = None;
                }
            }
            if drawing_mode && self.doc.tool != crate::jobs::Tool::Select {
                let tool = self.doc.tool;
                let use_snap = tool != crate::jobs::Tool::Pencil;
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.draw_pts = vec![screen_to_area(p, use_snap)];
                    }
                }
                if resp.dragged_by(egui::PointerButton::Primary) && !self.draw_pts.is_empty() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        let a = screen_to_area(p, use_snap);
                        if tool == crate::jobs::Tool::Pencil {
                            if geom::dist(*self.draw_pts.last().unwrap(), a) > 0.3 {
                                self.draw_pts.push(a);
                            }
                        } else if self.draw_pts.len() == 1 {
                            self.draw_pts.push(a);
                        } else {
                            self.draw_pts[1] = a;
                        }
                    }
                }
                if resp.drag_stopped_by(egui::PointerButton::Primary) && !self.draw_pts.is_empty() {
                    if let Some(sh) =
                        crate::jobs::Shape::from_drag(tool, &self.draw_pts, self.doc.grid_step, self.doc.grid_labels)
                    {
                        self.commit_history();
                        self.doc.shapes.push(sh);
                        self.doc.show_drawing = true;
                    }
                    self.draw_pts.clear();
                }
            }

            // щелчок по элементу листа — выделить его (Delete удалит именно его)
            if !drawing_mode && self.doc.mode != Mode::Calibrate && resp.clicked() {
                self.selected_elem = None;
                if let (Some(pp), Some(page)) = (resp.interact_pointer_pos(), self.pages.get(self.page)) {
                    let a = screen_to_area(pp, false);
                    let hit = |b: &[f64; 4]| a.0 >= b[0] - 2.0 && a.0 <= b[2] + 2.0 && a.1 >= b[1] - 2.0 && a.1 <= b[3] + 2.0;
                    let active = jobs::Elem::of_mode(self.doc.mode);
                    self.selected_elem = page
                        .boxes
                        .iter()
                        .find(|(e, b)| Some(*e) == active && hit(b))
                        .or_else(|| page.boxes.iter().rev().find(|(_, b)| hit(b)))
                        .map(|(e, _)| *e);
                }
            }
            // перетаскивание элементов листа (текст, таблица, SVG, рисование)
            if !drawing_mode && self.doc.mode != Mode::Calibrate && resp.drag_started_by(egui::PointerButton::Primary) {
                if let (Some(pp), Some(page)) = (resp.interact_pointer_pos(), self.pages.get(self.page)) {
                    let a = screen_to_area(pp, false);
                    let hit = |b: &[f64; 4]| a.0 >= b[0] - 2.0 && a.0 <= b[2] + 2.0 && a.1 >= b[1] - 2.0 && a.1 <= b[3] + 2.0;
                    let active = jobs::Elem::of_mode(self.doc.mode);
                    let found = page
                        .boxes
                        .iter()
                        .find(|(e, b)| Some(*e) == active && hit(b))
                        .or_else(|| page.boxes.iter().rev().find(|(_, b)| hit(b)))
                        .copied();
                    if let Some((elem, b)) = found {
                        let start = match elem {
                            jobs::Elem::Text => self.doc.text_pos,
                            jobs::Elem::Table => self.doc.table_pos.unwrap_or((b[0], b[1])),
                            jobs::Elem::Image => self.doc.svg_pos.unwrap_or((b[0], b[1])),
                            jobs::Elem::Drawing => (0.0, 0.0),
                        };
                        self.commit_history();
                        self.selected_elem = Some(elem);
                        self.moving = Some(Moving { elem, grab: a, start, applied: (0.0, 0.0) });
                        match elem {
                            jobs::Elem::Text => self.doc.mode = Mode::Text,
                            jobs::Elem::Table => self.doc.mode = Mode::Table,
                            jobs::Elem::Image => self.doc.mode = Mode::Image,
                            jobs::Elem::Drawing => {}
                        }
                    }
                }
            }
            if let Some(mv) = self.moving.as_mut() {
                if let Some(pp) = resp.interact_pointer_pos() {
                    let a = screen_to_area(pp, false);
                    let d = ((a.0 - mv.grab.0).round(), (a.1 - mv.grab.1).round());
                    let np = (mv.start.0 + d.0, mv.start.1 + d.1);
                    match mv.elem {
                        jobs::Elem::Text => self.doc.text_pos = np,
                        jobs::Elem::Table => self.doc.table_pos = Some(np),
                        jobs::Elem::Image => self.doc.svg_pos = Some(np),
                        jobs::Elem::Drawing => {
                            let inc = (d.0 - mv.applied.0, d.1 - mv.applied.1);
                            if inc != (0.0, 0.0) {
                                for sh in &mut self.doc.shapes {
                                    sh.translate(inc.0, inc.1);
                                }
                                mv.applied = d;
                            }
                        }
                    }
                }
                if !resp.dragged() {
                    self.moving = None;
                }
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            } else {
                self.pan += pan_delta;
            }

            let painter = painter.with_clip_rect(rect);
            // --- стол Bambu Lab A1: рамка нагревателя + текстурированная пластина PEI ---
            let bed = Rect::from_two_pos(to(0.0, 0.0), to(BED_W, BED_H));
            let frame = bed.expand(mm(9.0));
            painter.rect_filled(frame.translate(Vec2::new(0.0, mm(2.0))), mm(8.0), Color32::from_black_alpha(90));
            painter.rect_filled(frame, mm(8.0), Color32::from_rgb(58, 62, 68));
            painter.rect_stroke(frame, mm(8.0), Stroke::new(1.0_f32, Color32::from_rgb(84, 89, 96)), StrokeKind::Inside);
            // штифты-упоры пластины сзади
            for x in [20.0, BED_W - 20.0] {
                painter.circle_filled(to(x, BED_H + 4.5), mm(2.2), Color32::from_rgb(120, 125, 132));
            }
            let tex = self.plate_texture(ui.ctx());
            painter.image(tex, bed, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            painter.rect_stroke(bed, mm(3.0), Stroke::new(1.0_f32, Color32::from_rgb(96, 82, 52)), StrokeKind::Inside);
            // сетка 10 мм / 50 мм
            let fine = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 220, 150, if s > 2.0 { 16 } else { 8 }));
            let bold = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 220, 150, 30));
            let mut g = 0.0;
            while g <= BED_W {
                let st = if (g as i32) % 50 == 0 { bold } else { fine };
                painter.line_segment([to(g, 0.0), to(g, BED_H)], st);
                painter.line_segment([to(0.0, g), to(BED_W, g)], st);
                g += 10.0;
            }
            // линейки: по переднему (низ) и левому краю
            let ruler = Color32::from_rgb(150, 156, 166);
            let fsize = (2.6 * s).clamp(9.0, 13.0);
            let label_step = if s > 4.0 { 10 } else if s > 2.0 { 20 } else { 50 };
            for v in (0..=256).step_by(5) {
                let len = if v % 10 == 0 { 3.0 } else { 1.6 };
                let a = to(v as f64, -9.0);
                painter.line_segment([a, a + Vec2::new(0.0, mm(len))], Stroke::new(1.0_f32, ruler));
                let b = to(-9.0, v as f64);
                painter.line_segment([b, b - Vec2::new(mm(len), 0.0)], Stroke::new(1.0_f32, ruler));
                if v % label_step == 0 {
                    painter.text(a + Vec2::new(0.0, mm(len) + 2.0), Align2::CENTER_TOP, v.to_string(), FontId::proportional(fsize), ruler);
                    painter.text(b - Vec2::new(mm(len) + 3.0, 0.0), Align2::RIGHT_CENTER, v.to_string(), FontId::proportional(fsize), ruler);
                }
            }
            painter.text(to(BED_W / 2.0, -9.0) + Vec2::new(0.0, mm(3.0) + fsize + 6.0), Align2::CENTER_TOP,
                "ПЕРЕД (к вам) · мм", FontId::proportional(fsize), DIM);
            painter.text(to(BED_W / 2.0, BED_H + 9.0) - Vec2::new(0.0, 4.0), Align2::CENTER_BOTTOM,
                "ЗАД · Bambu Lab A1 · 256 × 256 мм", FontId::proportional(fsize), DIM);
            // зона, куда дотягивается кончик ручки
            let reach = self.cfg.pen_reach();
            if reach != [0.0, 0.0, BED_W, BED_H] {
                let rr = Rect::from_two_pos(to(reach[0], reach[1]), to(reach[2], reach[3]));
                painter.rect_filled(rr, 6.0, Color32::from_rgba_unmultiplied(0, 174, 66, 6));
                for seg in dashed_rect(rr) {
                    painter.add(egui::Shape::dashed_line(&seg, Stroke::new(1.5_f32, ACCENT.gamma_multiply(0.8)), 8.0, 5.0));
                }
                painter.text(
                    rr.left_bottom() + Vec2::new(6.0, -6.0),
                    Align2::LEFT_BOTTOM,
                    "зона ручки",
                    FontId::proportional(12.0),
                    ACCENT,
                );
            }
            // лист
            let paper = Rect::from_two_pos(to(ox, oy), to(ox + pw, oy + ph));
            painter.rect_filled(paper.translate(Vec2::splat(4.0)), 3.0, Color32::from_black_alpha(110));
            painter.rect_filled(paper, 2.0, Color32::from_rgb(250, 250, 247));
            let m = self.cfg.paper.margin;
            let area = Rect::from_two_pos(to(ox + m, oy + m), to(ox + pw - m, oy + ph - m));
            for seg in dashed_rect(area) {
                painter.add(egui::Shape::dashed_line(&seg, Stroke::new(1.0_f32, Color32::from_rgb(205, 210, 220)), 5.0, 4.0));
            }
            // части листа, куда ручка не дотягивается (например, A4 свисает со стола)
            {
                let (aw, ah) = self.cfg.paper.area();
                let u = self.cfg.usable_area();
                let m = self.cfg.paper.margin;
                let (al, at) = (ox + m, oy + ph - m);
                let a2s = |x: f64, y: f64| to(al + x, at - y);
                let shade = Color32::from_rgba_unmultiplied(220, 60, 60, 40);
                let mut biggest: Option<Rect> = None;
                for (x0, y0, x1, y1) in [
                    (0.0, 0.0, aw, u[1]),
                    (0.0, u[3], aw, ah),
                    (0.0, u[1], u[0], u[3]),
                    (u[2], u[1], aw, u[3]),
                ] {
                    if x1 - x0 > 0.5 && y1 - y0 > 0.5 {
                        let r = Rect::from_two_pos(a2s(x0, y0), a2s(x1, y1));
                        painter.rect_filled(r, 0.0, shade);
                        if biggest.is_none_or(|b| b.area() < r.area()) {
                            biggest = Some(r);
                        }
                    }
                }
                if let Some(r) = biggest.filter(|r| r.height() > 18.0 && r.width() > 120.0) {
                    painter.text(r.center(), Align2::CENTER_CENTER, "сюда ручка не дотягивается",
                        FontId::proportional(12.5), Color32::from_rgb(200, 70, 70));
                }
            }
            // где класть лист: расстояния от левого и переднего края стола
            let guide = Color32::from_rgb(255, 176, 64);
            let gs = Stroke::new(1.5_f32, guide);
            let gf = FontId::proportional((2.8 * s).clamp(10.0, 14.0));
            if ox > 0.5 {
                let y = oy + ph * 0.12;
                painter.add(egui::Shape::dashed_line(&[to(0.0, y), to(ox, y)], gs, 5.0, 3.0));
                painter.text(to(ox / 2.0, y) - Vec2::new(0.0, 3.0), Align2::CENTER_BOTTOM, format!("{ox:.0} мм"), gf.clone(), guide);
            }
            if oy > 0.5 {
                let x = ox + pw * 0.12;
                painter.add(egui::Shape::dashed_line(&[to(x, 0.0), to(x, oy)], gs, 5.0, 3.0));
                painter.text(to(x, oy / 2.0) + Vec2::new(5.0, 0.0), Align2::LEFT_CENTER, format!("{oy:.0} мм"), gf.clone(), guide);
            }
            painter.text(
                paper.left_bottom() + Vec2::new(4.0, -4.0),
                Align2::LEFT_BOTTOM,
                {
                    let fx = if ox >= 0.0 { format!("{ox:.0} мм от левого края") } else { format!("свисает на {:.0} мм слева", -ox) };
                    let fy = if oy >= 0.0 { format!("{oy:.0} мм от переднего края") } else { format!("свисает на {:.0} мм спереди", -oy) };
                    let back = oy + ph - BED_H;
                    let fb = if back > 0.5 { format!(", на {back:.0} мм сзади") } else { String::new() };
                    format!("лист {pw:.0}×{ph:.0}: {fx}, {fy}{fb}")
                },
                FontId::proportional((2.2 * s).clamp(9.0, 12.5)),
                Color32::from_rgb(150, 120, 60),
            );

            // штрихи
            let ink = Color32::from_rgb(self.prefs.ink[0], self.prefs.ink[1], self.prefs.ink[2]);
            let width = (0.3 * s).clamp(0.8, 5.0);
            let mut head: Option<Pos2> = None;
            if let Some(page) = self.pages.get(self.page) {
                // рисуем ровно то, что сделает G-code: линии там, где ручка по Z на бумаге
                let total: f64 = page.segs.iter().filter(|s| s.down).map(|s| geom::dist(s.a, s.b)).sum::<f64>().max(1e-9);
                let limit = self.sim as f64 * total;
                let mut acc = 0.0;
                let travel = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(230, 70, 70, 150));
                let mut poly: Vec<Pos2> = Vec::new();
                let flush = |poly: &mut Vec<Pos2>| {
                    if poly.len() >= 2 {
                        painter.add(egui::Shape::line(std::mem::take(poly), Stroke::new(width, ink)));
                    }
                    poly.clear();
                };
                for sg in &page.segs {
                    let (a, b) = (to(sg.a.0, sg.a.1), to(sg.b.0, sg.b.1));
                    if !sg.down {
                        flush(&mut poly);
                        if self.prefs.show_travel && a.distance(b) > 1.0 && self.sim >= 1.0 {
                            painter.add(egui::Shape::dashed_line(&[a, b], travel, 4.0, 4.0));
                        }
                        continue;
                    }
                    let d = geom::dist(sg.a, sg.b);
                    if poly.last().is_none_or(|l| l.distance(a) > 0.01) {
                        flush(&mut poly);
                        poly.push(a);
                    }
                    if acc + d > limit {
                        let p = a + (b - a) * ((limit - acc) / d.max(1e-9)) as f32;
                        poly.push(p);
                        head = Some(p);
                        break;
                    }
                    acc += d;
                    poly.push(b);
                }
                flush(&mut poly);
                if let Some(e) = &page.error {
                    banner(&painter, rect, &format!("⚠ {e}"));
                } else if let Some(w) = &page.warning {
                    banner_c(&painter, rect, &format!("⚠ {w}"), Color32::from_rgba_unmultiplied(150, 110, 20, 235));
                }
            } else if let Some(e) = &self.build_error {
                banner(&painter, rect, &format!("⚠ {e}"));
            } else {
                let msg = match self.doc.mode {
                    Mode::Image => "Выберите SVG-файл слева",
                    Mode::Draw => "Рисуйте мышью прямо на листе или нажмите «Тест масштаба»",
                    _ => "Документ пуст — добавьте текст слева",
                };
                painter.text(paper.center(), Align2::CENTER_CENTER, msg, FontId::proportional(16.0), Color32::from_rgb(160, 165, 175));
            }
            // рамки элементов листа: активный — зелёный с подписью, под курсором — тонкий
            if self.doc.mode != Mode::Calibrate {
                if let Some(page) = self.pages.get(self.page) {
                    let active = self.moving.as_ref().map(|m| m.elem).or(jobs::Elem::of_mode(self.doc.mode));
                    let hover = resp.hover_pos().map(|p| screen_to_area(p, false));
                    for (elem, b) in &page.boxes {
                        let r = Rect::from_two_pos(area_to_screen((b[0], b[1])), area_to_screen((b[2], b[3]))).expand(4.0);
                        if Some(*elem) == self.selected_elem && !drawing_mode {
                            let sel = Color32::from_rgb(255, 176, 64);
                            painter.rect_stroke(r.expand(3.0), 4.0, Stroke::new(2.0_f32, sel), StrokeKind::Outside);
                            let g = painter.layout_no_wrap(format!("Выбрано: {} · Delete — удалить", elem.name()), FontId::proportional(12.0), Color32::BLACK);
                            let lr = Rect::from_min_size(r.left_bottom() + Vec2::new(0.0, 6.0), g.size() + Vec2::new(10.0, 6.0));
                            painter.rect_filled(lr, 5.0, sel);
                            painter.galley(lr.min + Vec2::new(5.0, 3.0), g, Color32::BLACK);
                        }
                        let is_hover = hover.is_some_and(|a| a.0 >= b[0] && a.0 <= b[2] && a.1 >= b[1] && a.1 <= b[3]);
                        auto::track_rect(r.intersect(rect), &format!("элемент: {}", elem.name()), "element");
                        if Some(*elem) == active {
                            for seg in dashed_rect(r) {
                                painter.add(egui::Shape::dashed_line(&seg, Stroke::new(1.5_f32, ACCENT), 6.0, 4.0));
                            }
                            let hint = if *elem == jobs::Elem::Drawing && drawing_mode {
                                format!("{} · Delete — удалить всё нарисованное", elem.name())
                            } else {
                                format!("{} · перетащите, чтобы сдвинуть · Delete — удалить", elem.name())
                            };
                            let g = painter.layout_no_wrap(hint, FontId::proportional(12.0), Color32::WHITE);
                            let lr = Rect::from_min_size(r.left_top() - Vec2::new(0.0, g.size().y + 8.0), g.size() + Vec2::new(10.0, 6.0));
                            painter.rect_filled(lr, 5.0, ACCENT_DIM);
                            painter.galley(lr.min + Vec2::new(5.0, 3.0), g, Color32::WHITE);
                        } else if is_hover && !drawing_mode {
                            painter.rect_stroke(r, 3.0, Stroke::new(1.0_f32, ACCENT.gamma_multiply(0.6)), StrokeKind::Outside);
                        }
                        if is_hover && !drawing_mode && self.moving.is_none() {
                            ctx.set_cursor_icon(egui::CursorIcon::Grab);
                        }
                    }
                }
            }

            // выбранная фигура
            if drawing_mode {
                if let (Some(i), Some(font)) = (self.selected_shape, self.font.as_ref()) {
                    if let Some(sh) = self.doc.shapes.get(i) {
                        for p in sh.paths(font) {
                            let pts: Vec<Pos2> = p.iter().map(|&q| area_to_screen(q)).collect();
                            painter.add(egui::Shape::line(pts, Stroke::new(width.max(2.5), Color32::from_rgb(255, 176, 64))));
                        }
                        if let Some(b) = geom::bbox(&sh.paths(font)) {
                            let r = Rect::from_two_pos(area_to_screen((b[0], b[1])), area_to_screen((b[2], b[3]))).expand(6.0);
                            for seg in dashed_rect(r) {
                                painter.add(egui::Shape::dashed_line(&seg, Stroke::new(1.0_f32, Color32::from_rgb(255, 176, 64)), 4.0, 3.0));
                            }
                            painter.text(r.left_top() - Vec2::new(0.0, 4.0), Align2::LEFT_BOTTOM,
                                format!("{} · Delete — удалить", sh.describe()), FontId::proportional(12.0), Color32::from_rgb(255, 176, 64));
                        }
                    }
                }
            }
            // фигура, которую сейчас рисуют + размеры + координаты курсора
            if drawing_mode && self.doc.tool != crate::jobs::Tool::Select {
                let tip = Color32::from_rgb(255, 176, 64);
                if !self.draw_pts.is_empty() {
                    if let (Some(sh), Some(font)) = (
                        crate::jobs::Shape::from_drag(self.doc.tool, &self.draw_pts, self.doc.grid_step, self.doc.grid_labels),
                        self.font.as_ref(),
                    ) {
                        for p in sh.paths(font) {
                            let pts: Vec<Pos2> = p.iter().map(|&q| area_to_screen(q)).collect();
                            painter.add(egui::Shape::line(pts, Stroke::new(width.max(1.5), tip)));
                        }
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let label = sh.describe();
                            let g = painter.layout_no_wrap(label, FontId::proportional(14.0), Color32::WHITE);
                            let r = Rect::from_min_size(pos + Vec2::new(16.0, 14.0), g.size() + Vec2::new(14.0, 8.0));
                            painter.rect_filled(r, 6.0, Color32::from_rgba_unmultiplied(20, 22, 26, 230));
                            painter.galley(r.min + Vec2::new(7.0, 4.0), g, Color32::WHITE);
                        }
                    }
                } else if let Some(pos) = resp.hover_pos() {
                    let (x, y) = screen_to_area(pos, self.doc.tool != crate::jobs::Tool::Pencil);
                    let p = area_to_screen((x, y));
                    painter.circle_stroke(p, 5.0, Stroke::new(1.5_f32, tip));
                    painter.line_segment([p - Vec2::new(9.0, 0.0), p + Vec2::new(9.0, 0.0)], Stroke::new(1.0_f32, tip));
                    painter.line_segment([p - Vec2::new(0.0, 9.0), p + Vec2::new(0.0, 9.0)], Stroke::new(1.0_f32, tip));
                    let g = painter.layout_no_wrap(format!("x {x:.0}  y {y:.0} мм"), FontId::proportional(12.5), Color32::WHITE);
                    let r = Rect::from_min_size(pos + Vec2::new(14.0, 12.0), g.size() + Vec2::new(12.0, 6.0));
                    painter.rect_filled(r, 6.0, Color32::from_rgba_unmultiplied(20, 22, 26, 210));
                    painter.galley(r.min + Vec2::new(6.0, 3.0), g, Color32::WHITE);
                    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                }
            }

            if let Some(h) = head {
                painter.circle_filled(h, 7.0, ACCENT.gamma_multiply(0.35));
                painter.circle_filled(h, 3.5, ACCENT);
            }

            // масштабная полоска в правом нижнем углу
            {
                let target_px = 120.0;
                let len = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0]
                    .into_iter()
                    .min_by(|a, b| ((a * s) - target_px).abs().total_cmp(&((b * s) - target_px).abs()))
                    .unwrap_or(10.0);
                let w = len * s;
                let r = Rect::from_min_max(
                    Pos2::new(rect.right() - w - 34.0, rect.bottom() - 46.0),
                    Pos2::new(rect.right() - 14.0, rect.bottom() - 14.0),
                );
                painter.rect_filled(r, 9.0, Color32::from_rgba_unmultiplied(30, 34, 41, 235));
                let y = r.bottom() - 11.0;
                let (x0, x1) = (r.left() + 10.0, r.left() + 10.0 + w);
                let st = Stroke::new(2.0_f32, Color32::WHITE);
                painter.line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], st);
                painter.line_segment([Pos2::new(x0, y - 5.0), Pos2::new(x0, y + 1.0)], st);
                painter.line_segment([Pos2::new(x1, y - 5.0), Pos2::new(x1, y + 1.0)], st);
                painter.text(Pos2::new((x0 + x1) / 2.0, y - 6.0), Align2::CENTER_BOTTOM, format!("{len:.0} мм"),
                    FontId::proportional(12.5), Color32::WHITE);
            }

            // статистика — плашки в левом нижнем углу превью
            if let Some(p) = self.pages.get(self.page) {
                let chips = [
                    (fmt_time(p.stats.secs), "время"),
                    (format!("{:.1} м", p.stats.draw_mm / 1000.0), "линий"),
                    (p.stats.lifts.to_string(), "подъёмов"),
                    (format!("{}×{}", self.cfg.paper.width, self.cfg.paper.height), "мм"),
                ];
                let mut x = rect.left() + 14.0;
                let y = rect.bottom() - 14.0;
                for (val, label) in chips {
                    let gv = painter.layout_no_wrap(val, FontId::new(15.0, egui::FontFamily::Name("bold".into())), Color32::WHITE);
                    let gl = painter.layout_no_wrap(label.to_string(), FontId::proportional(12.5), DIM);
                    let w = gv.size().x + gl.size().x + 30.0;
                    let r = Rect::from_min_max(Pos2::new(x, y - 32.0), Pos2::new(x + w, y));
                    painter.rect_filled(r, 9.0, Color32::from_rgba_unmultiplied(30, 34, 41, 235));
                    painter.galley(Pos2::new(x + 11.0, r.center().y - gv.size().y / 2.0), gv.clone(), Color32::WHITE);
                    painter.galley(Pos2::new(x + 19.0 + gv.size().x, r.center().y - gl.size().y / 2.0), gl, DIM);
                    x += w + 8.0;
                }
            }

            // анимация
            if self.playing {
                let secs = self.pages.get(self.page).map(|p| p.stats.secs).unwrap_or(1.0).max(1.0) as f32;
                let dt = ui.input(|i| i.stable_dt).min(0.1);
                self.sim += dt * self.prefs.sim_speed / secs;
                if self.sim >= 1.0 {
                    self.sim = 1.0;
                    self.playing = false;
                }
                ctx.request_repaint();
            }
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Bambu Connect могли установить, пока программа открыта — перепроверяем раз в 3 с
        let now = ctx.input(|i| i.time);
        if !self.connect_ok && now - self.connect_checked > 3.0 {
            self.connect_checked = now;
            self.connect_ok = connect::is_installed();
        }
        if !self.connect_ok {
            ctx.request_repaint_after(std::time::Duration::from_secs(3));
        }
        // перетащенные файлы
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(f) = dropped.into_iter().find_map(|f| f.path) {
            self.open_path(&f);
            self.notify(ctx, format!("Открыт {}", file_name(&f)), false);
        }
        // результат фоновой отправки
        if let Some(rx) = &self.job_rx {
            if let Ok(r) = rx.try_recv() {
                self.job_rx = None;
                match r {
                    Ok(m) => self.notify(ctx, m, false),
                    Err(e) => self.notify(ctx, e, true),
                }
            }
        }
        if self.font_rx.is_some() {
            if let Some(rx) = &self.font_rx {
                if let Ok(list) = rx.try_recv() {
                    self.font_list = list;
                    self.font_rx = None;
                }
            }
        }

        self.hotkeys(ctx);
        if self.confirm_clear {
            egui::Window::new("Очистить лист?")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Будут удалены текст, таблица, рисунок и всё нарисованное.");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let yes = ui.add(egui::Button::new(RichText::new("Да, очистить").color(Color32::WHITE)).fill(DANGER));
                        auto::track(&yes, "Да, очистить", "button");
                        if yes.clicked() {
                            self.clear_all();
                            self.confirm_clear = false;
                        }
                        if tb(ui, "Отмена").clicked() {
                            self.confirm_clear = false;
                        }
                    });
                });
        }

        self.top_bar(ctx);
        self.status_bar(ctx);
        self.editor_panel(ctx);
        self.settings_panel(ctx);
        self.rebuild_if_needed();
        self.preview(ctx);
        self.save_cfg_if_changed(ctx);

        // подсветка при перетаскивании файла
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let r = ctx.screen_rect();
            let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            p.rect_filled(r, 0.0, Color32::from_black_alpha(170));
            p.rect_stroke(r.shrink(20.0), 16.0, Stroke::new(3.0_f32, ACCENT), StrokeKind::Inside);
            p.text(r.center(), Align2::CENTER_CENTER, "Отпустите файл, чтобы открыть", FontId::proportional(28.0), Color32::WHITE);
        }

        if self.show_keys {
            let mut open = true;
            egui::Window::new("⌨ Горячие клавиши").open(&mut open).collapsible(false).resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                    egui::Grid::new("keys").striped(true).spacing(Vec2::new(24.0, 6.0)).show(ui, |ui| {
                        for (k, d) in HOTKEYS {
                            ui.label(RichText::new(*k).strong().color(ACCENT));
                            ui.label(*d);
                            ui.end_row();
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new("Работают в любой раскладке (Ctrl+Я = Ctrl+Z). Не срабатывают, пока курсор в поле ввода.").color(DIM).small());
                });
            self.show_keys = open;
        }

        self.track_history(ctx);
        self.automation(ctx);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        auto::clear();
        if let Some(evs) = self.input_queue.pop_front() {
            raw.events.extend(evs);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.launch.persistent() {
            return;
        }
        eframe::set_value(storage, DOC_KEY, &self.doc);
        eframe::set_value(storage, UI_KEY, &self.prefs);
    }
}

// ---------------- автоматизация (для тестов) ----------------

impl App {
    fn automation(&mut self, ctx: &egui::Context) {
        self.frames += 1;

        // пришёл скриншот
        let img = ctx.input(|i| {
            i.events.iter().rev().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = img {
            for (path, tx) in self.shots.drain(..) {
                let v = match auto::save_png(&path, &img) {
                    Ok(()) => json!({"ok": true, "path": path, "width": img.size[0], "height": img.size[1]}),
                    Err(e) => json!({"ok": false, "error": e.to_string()}),
                };
                if let Some(tx) = tx {
                    let _ = tx.send(v);
                }
            }
            if self.quit_after_shot {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        } else if !self.shots.is_empty() {
            // запрос мог потеряться (окно не перерисовалось) — повторяем
            self.shot_wait += 1;
            if self.shot_wait % 30 == 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            }
            ctx.request_repaint();
        }

        // ответы после выполнения очереди ввода
        if self.input_queue.is_empty() {
            for tx in self.after_input.drain(..) {
                let _ = tx.send(json!({"ok": true}));
            }
        } else {
            ctx.request_repaint();
        }

        // --screenshot из командной строки
        if let Some(p) = self.launch.screenshot.clone() {
            if !self.cli_shot_started {
                if self.font_rx.is_none() && self.frames > 8 {
                    self.cli_shot_started = true;
                    self.quit_after_shot = true;
                    self.request_shot(ctx, p, None);
                } else {
                    ctx.request_repaint();
                }
            }
        }

        let reqs: Vec<auto::Request> = self.auto_rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for r in reqs {
            self.handle_request(ctx, r);
        }
    }

    fn request_shot(&mut self, ctx: &egui::Context, path: PathBuf, tx: Option<Sender<Value>>) {
        self.shots.push((path, tx));
        self.shot_wait = 0;
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        ctx.request_repaint();
    }

    fn queue_click(&mut self, pos: Pos2, double: bool) {
        let btn = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.input_queue.push_back(vec![egui::Event::PointerMoved(pos)]);
        let n = if double { 2 } else { 1 };
        for _ in 0..n {
            self.input_queue.push_back(vec![btn(true)]);
            self.input_queue.push_back(vec![btn(false)]);
        }
        self.input_queue.push_back(vec![]);
        self.input_queue.push_back(vec![]);
    }

    fn target_pos(&self, b: &Value) -> Result<Pos2, Value> {
        if let Some(l) = b["label"].as_str() {
            let idx = b["index"].as_u64().unwrap_or(0) as usize;
            return match auto::find(l, idx) {
                Some(w) if !w.visible => Err(json!({
                    "ok": false,
                    "error": format!("виджет «{l}» прокручен за пределы видимой области — сначала scroll над панелью"),
                    "y": w.rect.center().y,
                })),
                Some(w) => Ok(w.rect.center()),
                None => Err(json!({
                    "ok": false,
                    "error": format!("виджет «{l}» не найден"),
                    "available": auto::widgets().iter().map(|w| w.label.clone()).collect::<Vec<_>>(),
                })),
            };
        }
        match (b["x"].as_f64(), b["y"].as_f64()) {
            (Some(x), Some(y)) => Ok(Pos2::new(x as f32, y as f32)),
            _ => Err(json!({"ok": false, "error": "нужен label или x,y"})),
        }
    }

    fn state_json(&self, ctx: &egui::Context) -> Value {
        let now = ctx.input(|i| i.time);
        let toast = self.toast.as_ref().filter(|t| now < t.until).map(|t| json!({"text": t.text, "error": t.error}));
        let sr = ctx.screen_rect();
        json!({
            "ok": true,
            "mode": self.doc.mode,
            "job_name": self.doc.job_name,
            "pages": self.pages.len(),
            "page": self.page + 1,
            "page_stats": self.pages.iter().map(|p| json!({
                "strokes": p.strokes.len(),
                "points": p.strokes.iter().map(|s| s.pts.len()).sum::<usize>(),
                "draw_m": (p.stats.draw_mm / 10.0).round() / 100.0,
                "travel_m": (p.stats.travel_mm / 10.0).round() / 100.0,
                "lifts": p.stats.lifts,
                "warning": p.warning,
                "preview_draw_m": (p.segs.iter().filter(|s| s.down).map(|s| geom::dist(s.a, s.b)).sum::<f64>() / 10.0).round() / 100.0,
                "preview_bbox": seg_bbox(&p.segs),
                "boxes": p.boxes.iter().map(|(e, b)| json!({"elem": e, "bbox": b})).collect::<Vec<_>>(),
                "secs": p.stats.secs.round(),
                "error": p.error,
            })).collect::<Vec<_>>(),
            "build_error": self.build_error,
            "toast": toast,
            "busy": self.job_rx.is_some(),
            "connect_installed": self.connect_ok,
            "fonts_loaded": self.font_rx.is_none(),
            "font_count": self.font_list.len(),
            "sim": self.sim,
            "zoom": self.zoom,
            "window": [sr.width(), sr.height()],
            "frames": self.frames,
            "config_path": self.cfg_path,
            "doc": self.doc,
            "config": self.cfg,
            "usable_area": self.cfg.usable_area(),
            "paper_origin": self.cfg.paper_origin(),
            "prefs": {"show_travel": self.prefs.show_travel, "show_settings": self.prefs.show_settings, "sim_speed": self.prefs.sim_speed, "ink": self.prefs.ink},
        })
    }

    fn handle_request(&mut self, ctx: &egui::Context, req: auto::Request) {
        let b = req.body;
        let tx = req.reply;
        let cmd = b["cmd"].as_str().unwrap_or("").to_string();
        let ok = |tx: Sender<Value>, v: Value| {
            let _ = tx.send(v);
        };
        match cmd.as_str() {
            "state" => ok(tx, self.state_json(ctx)),
            "widgets" => ok(tx, json!({"ok": true, "widgets": auto::widgets_json()})),
            "click" | "double_click" => match self.target_pos(&b) {
                Ok(p) => {
                    self.queue_click(p, cmd == "double_click");
                    self.after_input.push(tx);
                }
                Err(e) => ok(tx, e),
            },
            "type" => {
                let text = b["text"].as_str().unwrap_or("").to_string();
                self.input_queue.push_back(vec![egui::Event::Text(text)]);
                self.input_queue.push_back(vec![]);
                self.after_input.push(tx);
            }
            "key" => {
                let name = b["key"].as_str().unwrap_or("");
                let Some(key) = egui::Key::from_name(name) else {
                    return ok(tx, json!({"ok": false, "error": format!("неизвестная клавиша {name}")}));
                };
                let modifiers = egui::Modifiers {
                    ctrl: b["ctrl"].as_bool().unwrap_or(false),
                    command: b["ctrl"].as_bool().unwrap_or(false),
                    shift: b["shift"].as_bool().unwrap_or(false),
                    alt: b["alt"].as_bool().unwrap_or(false),
                    ..Default::default()
                };
                let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
                self.input_queue.push_back(vec![ev(true)]);
                self.input_queue.push_back(vec![ev(false)]);
                self.input_queue.push_back(vec![]);
                self.after_input.push(tx);
            }
            "scroll" => match self.target_pos(&b) {
                Ok(p) => {
                    let dy = b["dy"].as_f64().unwrap_or(-120.0) as f32;
                    self.input_queue.push_back(vec![egui::Event::PointerMoved(p)]);
                    for _ in 0..6 {
                        self.input_queue.push_back(vec![egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: Vec2::new(0.0, dy / 6.0),
                            modifiers: Default::default(),
                        }]);
                    }
                    for _ in 0..10 {
                        self.input_queue.push_back(vec![]);
                    }
                    self.after_input.push(tx);
                }
                Err(e) => ok(tx, e),
            },
            "drag" => {
                // {"cmd":"drag","from":{"x":..,"y":..},"to":{"x":..,"y":..},"button":"left|right","steps":12}
                let (Ok(a), Ok(z)) = (self.target_pos(&b["from"]), self.target_pos(&b["to"])) else {
                    return ok(tx, json!({"ok": false, "error": "нужны from{x,y} и to{x,y}"}));
                };
                let button = if b["button"] == "right" { egui::PointerButton::Secondary } else { egui::PointerButton::Primary };
                let steps = b["steps"].as_u64().unwrap_or(12).max(2) as usize;
                let btn = |pos, pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: Default::default() };
                self.input_queue.push_back(vec![egui::Event::PointerMoved(a)]);
                self.input_queue.push_back(vec![btn(a, true)]);
                for i in 1..=steps {
                    let p = a + (z - a) * (i as f32 / steps as f32);
                    self.input_queue.push_back(vec![egui::Event::PointerMoved(p)]);
                }
                self.input_queue.push_back(vec![btn(z, false)]);
                self.input_queue.push_back(vec![]);
                self.input_queue.push_back(vec![]);
                self.after_input.push(tx);
            }
            "area_to_screen" => {
                // мм рабочей области листа (Y вниз) -> экранные точки
                match (self.view_tf, b["x"].as_f64(), b["y"].as_f64()) {
                    (Some((center, s, cx, cy, ax0, atop)), Some(x), Some(y)) => {
                        let (bx, by) = (ax0 + x, atop - y);
                        let p = Pos2::new(center.x + ((bx - cx) as f32) * s, center.y - ((by - cy) as f32) * s);
                        ok(tx, json!({"ok": true, "x": p.x, "y": p.y}))
                    }
                    _ => ok(tx, json!({"ok": false, "error": "нужны x, y (и открытое превью)"})),
                }
            }
            "wait" => {
                for _ in 0..b["frames"].as_u64().unwrap_or(5) {
                    self.input_queue.push_back(vec![]);
                }
                self.after_input.push(tx);
            }
            "set_doc" => {
                self.commit_history();
                let mut v = serde_json::to_value(&self.doc).unwrap_or_default();
                auto::merge(&mut v, &b["doc"]);
                match serde_json::from_value(v) {
                    Ok(d) => {
                        self.doc = d;
                        self.input_queue.push_back(vec![]);
                        self.input_queue.push_back(vec![]);
                        self.after_input.push(tx);
                    }
                    Err(e) => ok(tx, json!({"ok": false, "error": e.to_string()})),
                }
            }
            "set_config" => {
                let mut v = serde_json::to_value(&self.cfg).unwrap_or_default();
                auto::merge(&mut v, &b["config"]);
                match serde_json::from_value(v) {
                    Ok(c) => {
                        self.cfg = c;
                        self.input_queue.push_back(vec![]);
                        self.input_queue.push_back(vec![]);
                        self.after_input.push(tx);
                    }
                    Err(e) => ok(tx, json!({"ok": false, "error": e.to_string()})),
                }
            }
            "open_file" => {
                let p = PathBuf::from(b["path"].as_str().unwrap_or(""));
                if !p.exists() {
                    return ok(tx, json!({"ok": false, "error": "файл не найден"}));
                }
                self.open_path(&p);
                self.input_queue.push_back(vec![]);
                self.input_queue.push_back(vec![]);
                self.after_input.push(tx);
            }
            "page" => {
                let n = b["page"].as_u64().unwrap_or(1).max(1) as usize - 1;
                self.page = n.min(self.pages.len().saturating_sub(1));
                self.input_queue.push_back(vec![]);
                self.after_input.push(tx);
            }
            "sim" => {
                self.sim = b["value"].as_f64().unwrap_or(1.0).clamp(0.0, 1.0) as f32;
                self.playing = false;
                self.input_queue.push_back(vec![]);
                self.after_input.push(tx);
            }
            "screenshot" => {
                self.shot_counter += 1;
                let path = b["path"].as_str().map(PathBuf::from).unwrap_or_else(|| {
                    std::env::temp_dir().join("bambu-plotter-test").join(format!("shot-{}.png", self.shot_counter))
                });
                self.request_shot(ctx, path, Some(tx));
            }
            "export" => {
                let path = b["path"].as_str().map(PathBuf::from).unwrap_or_else(|| {
                    std::env::temp_dir().join("bambu-plotter-test").join("export.gcode.3mf")
                });
                if let Some(d) = path.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                match self.export_to(&path) {
                    Ok(()) => ok(tx, inspect_3mf(&path)),
                    Err(e) => ok(tx, json!({"ok": false, "error": format!("{e:#}")})),
                }
            }
            "print" => {
                self.print(ctx);
                let t = self.toast.as_ref().map(|t| json!({"text": t.text, "error": t.error}));
                ok(tx, json!({"ok": true, "toast": t}));
            }
            "quit" => {
                ok(tx, json!({"ok": true}));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            other => ok(
                tx,
                json!({"ok": false, "error": format!("неизвестная команда «{other}»"),
                    "commands": ["state","widgets","click","double_click","type","key","scroll","wait","set_doc","set_config","open_file","page","sim","screenshot","export","print","quit"]}),
            ),
        }
        ctx.request_repaint();
    }
}

/// Проверка готового .gcode.3mf: состав архива, md5, заголовок и статистика G-code.
fn inspect_3mf(path: &Path) -> Value {
    let r = (|| -> anyhow::Result<Value> {
        let f = std::fs::File::open(path)?;
        let mut z = zip::ZipArchive::new(f)?;
        let names: Vec<String> = (0..z.len()).filter_map(|i| z.by_index(i).ok().map(|e| e.name().to_string())).collect();
        let mut gcode = String::new();
        std::io::Read::read_to_string(&mut z.by_name("Metadata/plate_1.gcode")?, &mut gcode)?;
        let mut pj = String::new();
        let _ = z.by_name("Metadata/plate_1.json").map(|mut e| std::io::Read::read_to_string(&mut e, &mut pj));
        let bbox = serde_json::from_str::<Value>(&pj).ok().map(|v| v["bbox_all"].clone()).unwrap_or(Value::Null);
        let mut md5s = String::new();
        std::io::Read::read_to_string(&mut z.by_name("Metadata/plate_1.gcode.md5")?, &mut md5s)?;
        let md5_ok = md5s.trim() == format!("{:X}", md5::compute(gcode.as_bytes()));
        let lines: Vec<&str> = gcode.lines().collect();
        let count = |p: &str| lines.iter().filter(|l| l.starts_with(p)).count();
        let extrude = lines.iter().any(|l| l.starts_with("G1") && l.contains(" E"));
        let heat = lines.iter().any(|l| (l.starts_with("M104") || l.starts_with("M109") || l.starts_with("M140")) && !l.contains("S0"));
        let non_ascii = gcode.chars().any(|c| !c.is_ascii());
        let (mut minx, mut miny, mut maxx, mut maxy, mut minz, mut maxz) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN, f64::MAX, f64::MIN);
        for l in &lines {
            if !l.starts_with("G1") {
                continue;
            }
            for w in l.split_whitespace() {
                let v = w[1..].parse::<f64>();
                match (w.as_bytes()[0], v) {
                    (b'X', Ok(v)) => {
                        minx = minx.min(v);
                        maxx = maxx.max(v)
                    }
                    (b'Y', Ok(v)) => {
                        miny = miny.min(v);
                        maxy = maxy.max(v)
                    }
                    (b'Z', Ok(v)) => {
                        minz = minz.min(v);
                        maxz = maxz.max(v)
                    }
                    _ => {}
                }
            }
        }
        Ok(json!({
            "ok": true,
            "path": path,
            "size_bytes": std::fs::metadata(path)?.len(),
            "entries": names,
            "md5_ok": md5_ok,
            "gcode_lines": lines.len(),
            "g1_moves": count("G1"),
            "has_g28": count("G28") > 0,
            "extrusion": extrude,
            "heating": heat,
            "non_ascii": non_ascii,
            "nozzle_x": [minx, maxx], "nozzle_y": [miny, maxy], "z": [minz, maxz],
            "bbox": bbox,
            "head": lines.iter().take(20).collect::<Vec<_>>(),
        }))
    })();
    r.unwrap_or_else(|e| json!({"ok": false, "error": format!("{e:#}")}))
}

// ---------------- оформление ----------------

fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, path) in [("segoe", r"C:\Windows\Fonts\segoeui.ttf"), ("segoe_b", r"C:\Windows\Fonts\segoeuib.ttf")] {
        if let Ok(data) = std::fs::read(path) {
            fonts.font_data.insert(name.into(), std::sync::Arc::new(egui::FontData::from_owned(data)));
        }
    }
    if fonts.font_data.contains_key("segoe") {
        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "segoe".into());
    }
    if fonts.font_data.contains_key("segoe_b") {
        fonts.families.insert(egui::FontFamily::Name("bold".into()), vec!["segoe_b".into(), "segoe".into()]);
    } else {
        let prop = fonts.families[&egui::FontFamily::Proportional].clone();
        fonts.families.insert(egui::FontFamily::Name("bold".into()), prop);
    }
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::from_rgb(17, 19, 23);
    v.faint_bg_color = Color32::from_rgb(34, 38, 46);
    v.selection.bg_fill = ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(44, 49, 58));
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(215, 219, 226));
    v.widgets.inactive.bg_fill = CARD_HI;
    v.widgets.inactive.weak_bg_fill = CARD_HI;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(225, 228, 234));
    v.widgets.hovered.bg_fill = Color32::from_rgb(52, 58, 69);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(52, 58, 69);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT_DIM);
    v.widgets.active.bg_fill = ACCENT_DIM;
    v.widgets.active.weak_bg_fill = ACCENT_DIM;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(7);
    }
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);

    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.slider_width = 170.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(15.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(15.0));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(12.5));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::proportional(22.0));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(14.0));
    });
}

// виджеты с регистрацией в реестре автоматизации
fn tb(ui: &mut Ui, text: impl AsRef<str>) -> egui::Response {
    let r = ui.button(text.as_ref());
    auto::track(&r, text.as_ref(), "button");
    r
}

fn tsb(ui: &mut Ui, text: impl AsRef<str>) -> egui::Response {
    let r = ui.small_button(text.as_ref());
    auto::track(&r, text.as_ref(), "button");
    r
}

fn tcb(ui: &mut Ui, v: &mut bool, text: &str) -> egui::Response {
    let r = ui.checkbox(v, text);
    auto::track(&r, text, "checkbox");
    r
}

fn tsl(ui: &mut Ui, selected: bool, text: impl AsRef<str>) -> egui::Response {
    let r = ui.selectable_label(selected, text.as_ref());
    auto::track(&r, text.as_ref(), "item");
    r
}

fn tg(ui: &mut Ui, text: &str) -> egui::Response {
    let r = ui.add(ghost_button(text));
    auto::track(&r, text, "button");
    r
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

fn logo(ui: &mut Ui) {
    let (r, p) = ui.allocate_painter(Vec2::new(34.0, 34.0), Sense::hover());
    let rect = r.rect;
    p.rect_filled(rect, 9.0, ACCENT);
    let a = rect.left_bottom() + Vec2::new(9.0, -9.0);
    let b = rect.right_top() + Vec2::new(-9.0, 9.0);
    p.line_segment([a, b], Stroke::new(3.5_f32, Color32::WHITE));
    p.circle_filled(a, 3.0, Color32::WHITE);
    ui.add_space(4.0);
    ui.vertical(|ui| {
        ui.add_space(9.0);
        ui.label(RichText::new("Bambu Plotter").size(18.0).family(egui::FontFamily::Name("bold".into())).color(Color32::WHITE));
        ui.add_space(-8.0);
        ui.label(RichText::new("студия для пера · A1").size(11.5).color(DIM));
    });
}

fn tab(ui: &mut Ui, selected: bool, label: &str) -> egui::Response {
    let text = RichText::new(label).size(15.0).color(if selected { Color32::WHITE } else { DIM });
    let b = egui::Button::new(text)
        .fill(if selected { CARD_HI } else { Color32::TRANSPARENT })
        .stroke(if selected { Stroke::new(1.0_f32, ACCENT_DIM) } else { Stroke::NONE })
        .corner_radius(9)
        .min_size(Vec2::new(0.0, 36.0));
    let r = ui.add(b);
    auto::track(&r, label, "tab");
    r
}

fn segment(ui: &mut Ui, selected: bool, label: &str) -> egui::Response {
    let b = egui::Button::new(RichText::new(label).color(if selected { Color32::WHITE } else { DIM }))
        .fill(if selected { ACCENT_DIM } else { CARD_HI })
        .corner_radius(7);
    let r = ui.add(b);
    auto::track(&r, label, "button");
    r
}

fn ghost_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).size(14.5)).fill(CARD).corner_radius(9).min_size(Vec2::new(0.0, 36.0))
}

fn card(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::default()
        .fill(CARD)
        .corner_radius(12)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(12.0).strong().color(DIM));
            ui.add_space(2.0);
            add(ui);
        });
    ui.add_space(10.0);
}

fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> egui::Response) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            add(ui);
        });
    });
}

fn banner(p: &egui::Painter, rect: Rect, text: &str) {
    banner_c(p, rect, text, Color32::from_rgba_unmultiplied(150, 40, 40, 235));
}

fn banner_c(p: &egui::Painter, rect: Rect, text: &str, fill: Color32) {
    let r = Rect::from_min_size(rect.left_top() + Vec2::new(16.0, 12.0), Vec2::new(rect.width() - 32.0, 0.0));
    let galley = p.layout(text.to_string(), FontId::proportional(14.5), Color32::WHITE, r.width() - 24.0);
    let box_r = Rect::from_min_size(r.min, Vec2::new(r.width(), galley.size().y + 18.0));
    p.rect_filled(box_r, 10.0, fill);
    p.galley(box_r.min + Vec2::new(12.0, 9.0), galley, Color32::WHITE);
}

fn dashed_rect(r: Rect) -> Vec<Vec<Pos2>> {
    vec![vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()]]
}

// ---------------- утилиты ----------------

fn normalize(mut rows: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    for r in &mut rows {
        r.resize(cols, String::new());
    }
    rows
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// Транслитерация в латиницу для имени файла на принтере: «Мой рисунок — стр. 2» -> «Moy_risunok_str_2».
fn translit(s: &str) -> String {
    let map = |c: char| -> &'static str {
        match c.to_lowercase().next().unwrap_or(c) {
            'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' => "e", 'ё' => "e", 'ж' => "zh",
            'з' => "z", 'и' => "i", 'й' => "y", 'к' => "k", 'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o",
            'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t", 'у' => "u", 'ф' => "f", 'х' => "h", 'ц' => "ts",
            'ч' => "ch", 'ш' => "sh", 'щ' => "sch", 'ъ' => "", 'ы' => "y", 'ь' => "", 'э' => "e", 'ю' => "yu",
            'я' => "ya",
            _ => "",
        }
    };
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            let t = map(c);
            if !t.is_empty() {
                if c.is_uppercase() {
                    let mut it = t.chars();
                    if let Some(f) = it.next() {
                        out.extend(f.to_uppercase());
                        out.push_str(it.as_str());
                    }
                } else {
                    out.push_str(t);
                }
            } else if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() { "plot".into() } else { out.chars().take(60).collect() }
}

fn safe_file_name(s: &str) -> String {
    s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect()
}

fn fmt_time(secs: f64) -> String {
    let m = (secs / 60.0).round() as u64;
    if m >= 60 {
        format!("{} ч {} мин", m / 60, m % 60)
    } else if secs < 60.0 {
        format!("{:.0} с", secs)
    } else {
        format!("{m} мин")
    }
}

/// Габариты линий, которые реально нарисует ручка (по разбору G-code).
fn seg_bbox(segs: &[crate::gcode::Seg]) -> Value {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for s in segs.iter().filter(|s| s.down) {
        for p in [s.a, s.b] {
            b[0] = b[0].min(p.0);
            b[1] = b[1].min(p.1);
            b[2] = b[2].max(p.0);
            b[3] = b[3].max(p.1);
        }
    }
    if b[0] > b[2] {
        return Value::Null;
    }
    json!(b.map(|v| (v * 100.0).round() / 100.0))
}
