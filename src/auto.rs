//! Автоматизация для тестов: локальный HTTP JSON API, реестр виджетов, параметры запуска.
//!
//! Запуск:  BambuPlotter.exe --automation 7878 [--offscreen]
//! Запрос:  POST http://127.0.0.1:7878/  {"cmd": "screenshot", "path": "C:/tmp/a.png"}
//! Команды: state, widgets, click, type, key, scroll, set_doc, set_config, open_file,
//!          page, sim, screenshot, export, print, quit

use eframe::egui::{self, Rect, Response};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

#[derive(Default, Clone)]
pub struct Launch {
    pub open: Option<PathBuf>,
    pub automation: Option<u16>,
    pub screenshot: Option<PathBuf>,
    pub mode: Option<String>,
    pub offscreen: bool,
    pub size: Option<(f32, f32)>,
    /// Постоянный тестовый профиль: сохраняет настройки/документ как обычный запуск, но в отдельной папке.
    pub profile: Option<String>,
}

impl Launch {
    /// Тестовый запуск: отдельные настройки и сохранённое состояние, чтобы не трогать данные пользователя.
    pub fn is_test(&self) -> bool {
        self.automation.is_some() || self.screenshot.is_some() || self.profile.is_some()
    }

    /// Сохранять ли состояние между запусками.
    pub fn persistent(&self) -> bool {
        !self.is_test() || self.profile.is_some()
    }

    /// Имя хранилища eframe (папка в %APPDATA%).
    pub fn storage_name(&self) -> String {
        match (&self.profile, self.is_test()) {
            (Some(p), _) => format!("Bambu Plotter Profile {p}"),
            (None, true) => "Bambu Plotter Test".into(),
            (None, false) => "Bambu Plotter".into(),
        }
    }

    pub fn parse(args: impl Iterator<Item = String>) -> Launch {
        let mut l = Launch::default();
        let mut it = args.peekable();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--automation" => {
                    let port = it.peek().and_then(|p| p.parse::<u16>().ok());
                    if port.is_some() {
                        it.next();
                    }
                    l.automation = Some(port.unwrap_or(7878));
                }
                "--screenshot" => l.screenshot = it.next().map(PathBuf::from),
                "--mode" => l.mode = it.next(),
                "--open" => l.open = it.next().map(PathBuf::from),
                "--offscreen" => l.offscreen = true,
                "--profile" => l.profile = it.next(),
                "--size" => {
                    l.size = it.next().and_then(|s| {
                        let (w, h) = s.split_once('x')?;
                        Some((w.parse().ok()?, h.parse().ok()?))
                    })
                }
                _ if !a.starts_with("--") => l.open = Some(PathBuf::from(a)),
                _ => {}
            }
        }
        l
    }
}

// ---------------- реестр виджетов (заполняется каждый кадр) ----------------

#[derive(Clone)]
pub struct Widget {
    pub label: String,
    pub kind: &'static str,
    pub rect: Rect,
    pub enabled: bool,
    pub visible: bool,
}

thread_local! {
    static WIDGETS: RefCell<Vec<Widget>> = const { RefCell::new(Vec::new()) };
}

pub fn track(r: &Response, label: &str, kind: &'static str) {
    let label = label.trim().to_string();
    // interact_rect — видимая (не прокрученная за край) часть виджета
    let visible = r.interact_rect.is_positive();
    let rect = if visible { r.interact_rect } else { r.rect };
    WIDGETS.with(|w| w.borrow_mut().push(Widget { label, kind, rect, enabled: r.enabled(), visible }));
}

/// Регистрация произвольной области (например, элемента листа на превью).
pub fn track_rect(rect: Rect, label: &str, kind: &'static str) {
    let visible = rect.is_positive();
    WIDGETS.with(|w| w.borrow_mut().push(Widget { label: label.to_string(), kind, rect, enabled: true, visible }));
}

pub fn clear() {
    WIDGETS.with(|w| w.borrow_mut().clear());
}

pub fn widgets() -> Vec<Widget> {
    WIDGETS.with(|w| w.borrow().clone())
}

pub fn widgets_json() -> Value {
    Value::Array(
        widgets()
            .iter()
            .map(|w| {
                json!({
                    "label": w.label, "kind": w.kind, "enabled": w.enabled, "visible": w.visible,
                    "x": w.rect.center().x.round(), "y": w.rect.center().y.round(),
                    "w": w.rect.width().round(), "h": w.rect.height().round(),
                })
            })
            .collect(),
    )
}

/// Ищем виджет: точное совпадение, затем без учёта регистра, затем по подстроке.
pub fn find(label: &str, index: usize) -> Option<Widget> {
    let all = widgets();
    let norm = |s: &str| s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let l = norm(label);
    let tiers: [Box<dyn Fn(&Widget) -> bool>; 3] = [
        Box::new(|w: &Widget| w.label == label),
        Box::new(|w: &Widget| norm(&w.label) == l),
        Box::new(|w: &Widget| norm(&w.label).contains(&l)),
    ];
    for t in tiers.iter() {
        let m: Vec<&Widget> = all.iter().filter(|w| t(w)).collect();
        if let Some(w) = m.get(index) {
            return Some((*w).clone());
        }
    }
    None
}

// ---------------- HTTP-сервер ----------------

pub struct Request {
    pub body: Value,
    pub reply: Sender<Value>,
}

pub fn serve(port: u16, ctx: egui::Context) -> Receiver<Request> {
    let (tx, rx) = channel::<Request>();
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("automation: не удалось открыть порт {port}: {e}");
                return;
            }
        };
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, tx, ctx);
            });
        }
    });
    rx
}

fn handle(stream: TcpStream, tx: Sender<Request>, ctx: egui::Context) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let body: Value = if body.is_empty() { json!({"cmd": "state"}) } else { serde_json::from_slice(&body).unwrap_or(json!({})) };
    let (rtx, rrx) = channel();
    let resp = if tx.send(Request { body, reply: rtx }).is_ok() {
        ctx.request_repaint();
        rrx.recv_timeout(Duration::from_secs(60)).unwrap_or(json!({"ok": false, "error": "timeout"}))
    } else {
        json!({"ok": false, "error": "app closed"})
    };
    let out = serde_json::to_vec_pretty(&resp).unwrap_or_default();
    let mut s = stream;
    write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        out.len()
    )?;
    s.write_all(&out)?;
    s.flush()
}

/// Глубокое слияние JSON (для set_doc / set_config).
pub fn merge(dst: &mut Value, patch: &Value) {
    match (dst, patch) {
        (Value::Object(d), Value::Object(p)) => {
            for (k, v) in p {
                merge(d.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (d, p) => *d = p.clone(),
    }
}

pub fn save_png(path: &std::path::Path, img: &egui::ColorImage) -> anyhow::Result<()> {
    let [w, h] = img.size;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in &img.pixels {
        rgba.extend_from_slice(&p.to_srgba_unmultiplied());
    }
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let f = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    Ok(())
}
