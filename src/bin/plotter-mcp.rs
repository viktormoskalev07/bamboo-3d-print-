//! MCP-сервер (stdio, JSON-RPC 2.0) для управления Bambu Plotter из Claude / других агентов.
//!
//! Подключение (Claude Code):  claude mcp add bambu-plotter -- J:\plotter\target\release\plotter-mcp.exe
//! Сервер сам запускает BambuPlotter в режиме автоматизации (по умолчанию окно за пределами экрана)
//! и пересылает команды в его локальный HTTP API.

use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const PORT: u16 = 7878;

fn main() {
    let mut srv = Server { child: None };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let id = msg.get("id").cloned();
        let method = msg["method"].as_str().unwrap_or("");
        let result = match method {
            "initialize" => Some(Ok(json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "bambu-plotter", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Управление приложением Bambu Plotter (рисование ручкой на Bambu Lab A1). \
                    Начните с launch, затем screenshot/state. Клик по кнопке — click с label (текст кнопки). \
                    Тестовый запуск не трогает настройки и документ пользователя."
            }))),
            "ping" => Some(Ok(json!({}))),
            "tools/list" => Some(Ok(json!({"tools": tools()}))),
            "tools/call" => Some(Ok(srv.call(&msg["params"]["name"], &msg["params"]["arguments"]))),
            _ if id.is_none() => None, // уведомления
            _ => Some(Err(json!({"code": -32601, "message": format!("method not found: {method}")}))),
        };
        if let (Some(id), Some(r)) = (id, result) {
            let resp = match r {
                Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
                Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
            };
            let _ = writeln!(out, "{resp}");
            let _ = out.flush();
        }
    }
    if let Some(mut c) = srv.child.take() {
        let _ = c.kill();
    }
}

fn tool(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": desc,
        "inputSchema": {"type": "object", "properties": props, "required": required}})
}

fn tools() -> Vec<Value> {
    let target = json!({
        "label": {"type": "string", "description": "Текст виджета (кнопки, вкладки, чекбокса), напр. «Таблица», «Печать», «ячейка 1:2»"},
        "index": {"type": "integer", "description": "Если совпадений несколько — номер (с 0)"},
        "x": {"type": "number"}, "y": {"type": "number"}
    });
    vec![
        tool("launch", "Запустить Bambu Plotter в тестовом режиме (если ещё не запущен). visible=true — показать окно на экране, иначе окно за пределами экрана. Тест использует копию настроек пользователя и не меняет его данные.",
            json!({"visible": {"type": "boolean"}, "width": {"type": "number"}, "height": {"type": "number"}, "restart": {"type": "boolean"}}), &[]),
        tool("screenshot", "Скриншот окна приложения (рендер egui, без посторонних окон). Возвращает картинку.",
            json!({"path": {"type": "string", "description": "куда сохранить PNG (необязательно)"}}), &[]),
        tool("state", "Состояние приложения: режим, страницы, статистика, ошибки, тост, документ, настройки.", json!({}), &[]),
        tool("widgets", "Список видимых интерактивных виджетов с подписями и координатами (точки).", json!({}), &[]),
        tool("click", "Клик по виджету (по подписи) или по координатам.", target.clone(), &[]),
        tool("double_click", "Двойной клик по виджету или координатам.", target.clone(), &[]),
        tool("type", "Ввести текст в виджет с фокусом (сначала click по полю).", json!({"text": {"type": "string"}}), &["text"]),
        tool("key", "Нажать клавишу (имена egui: Enter, Tab, Backspace, Escape, ArrowDown, A, …).",
            json!({"key": {"type": "string"}, "ctrl": {"type": "boolean"}, "shift": {"type": "boolean"}, "alt": {"type": "boolean"}}), &["key"]),
        tool("scroll", "Прокрутка колесом над виджетом/точкой (на превью — масштаб). dy<0 — приблизить.",
            {let mut t = target.clone(); t["dy"] = json!({"type": "number"}); t}, &[]),
        tool("set_doc", "Изменить документ (слияние JSON). Поля: mode (Text|Table|Image|Calibrate), job_name, text, align (0/1/2), table ([[..]]), table_header, svg_path, svg_fit, svg_scale, svg_rot, cal_step, cal_steps.",
            json!({"doc": {"type": "object"}}), &["doc"]),
        tool("set_config", "Изменить настройки (слияние JSON): paper{width,height,margin,origin_x,origin_y}, pen{z_up,z_down,offset_x,offset_y,draw_speed,travel_speed,z_speed,home,...}, text{font,size,line_spacing}, printer{method}.",
            json!({"config": {"type": "object"}}), &["config"]),
        tool("open_file", "Открыть файл .txt/.csv/.tsv/.svg как через «Открыть».", json!({"path": {"type": "string"}}), &["path"]),
        tool("page", "Перейти на страницу N (с 1).", json!({"page": {"type": "integer"}}), &["page"]),
        tool("sim", "Положение симуляции рисования 0..1.", json!({"value": {"type": "number"}}), &["value"]),
        tool("export", "Сохранить текущую страницу в .gcode.3mf и проверить файл (состав, md5, нагрев, экструзия, границы).",
            json!({"path": {"type": "string"}}), &[]),
        tool("print", "Нажать «Печать» (откроет Bambu Connect на компьютере пользователя!).", json!({}), &[]),
        tool("wait", "Подождать N кадров.", json!({"frames": {"type": "integer"}}), &[]),
        tool("quit", "Закрыть тестовое приложение.", json!({}), &[]),
    ]
}

struct Server {
    child: Option<Child>,
}

impl Server {
    fn call(&mut self, name: &Value, args: &Value) -> Value {
        let name = name.as_str().unwrap_or("");
        let r = match name {
            "launch" => self.launch(args),
            "quit" => {
                let r = send(&json!({"cmd": "quit"}));
                if let Some(mut c) = self.child.take() {
                    std::thread::sleep(Duration::from_millis(500));
                    let _ = c.kill();
                }
                r.map(|_| json!({"ok": true}))
            }
            _ => self.ensure().and_then(|_| {
                let mut body = if args.is_object() { args.clone() } else { json!({}) };
                body["cmd"] = json!(name);
                send(&body)
            }),
        };
        match r {
            Ok(v) if name == "screenshot" && v["ok"] == true => {
                let path = v["path"].as_str().unwrap_or("");
                match std::fs::read(path) {
                    Ok(png) => json!({"content": [
                        {"type": "image", "data": base64(&png), "mimeType": "image/png"},
                        {"type": "text", "text": format!("{}  ({}×{})", path, v["width"], v["height"])}
                    ]}),
                    Err(e) => text_result(&json!({"ok": false, "error": e.to_string()}), true),
                }
            }
            Ok(v) => {
                let err = v["ok"] == false;
                text_result(&v, err)
            }
            Err(e) => text_result(&json!({"ok": false, "error": e}), true),
        }
    }

    fn ensure(&mut self) -> Result<(), String> {
        if send(&json!({"cmd": "state"})).is_ok() {
            return Ok(());
        }
        self.launch(&json!({}))?;
        Ok(())
    }

    fn launch(&mut self, args: &Value) -> Result<Value, String> {
        if args["restart"].as_bool().unwrap_or(false) {
            let _ = send(&json!({"cmd": "quit"}));
            if let Some(mut c) = self.child.take() {
                std::thread::sleep(Duration::from_millis(600));
                let _ = c.kill();
            }
        }
        if let Ok(v) = send(&json!({"cmd": "state"})) {
            return Ok(json!({"ok": true, "already_running": true, "window": v["window"]}));
        }
        let exe = app_exe().ok_or("не найден BambuPlotter.exe рядом с plotter-mcp.exe")?;
        let mut cmd = Command::new(&exe);
        cmd.arg("--automation").arg(PORT.to_string());
        if !args["visible"].as_bool().unwrap_or(false) {
            cmd.arg("--offscreen");
        }
        if let (Some(w), Some(h)) = (args["width"].as_f64(), args["height"].as_f64()) {
            cmd.arg("--size").arg(format!("{w}x{h}"));
        }
        let child = cmd.spawn().map_err(|e| format!("не удалось запустить {}: {e}", exe.display()))?;
        self.child = Some(child);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            if let Ok(v) = send(&json!({"cmd": "state"})) {
                if v["fonts_loaded"] == true {
                    return Ok(json!({"ok": true, "exe": exe, "window": v["window"]}));
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err("приложение не ответило за 20 с".into())
    }
}

fn app_exe() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BAMBU_PLOTTER_EXE") {
        return Some(PathBuf::from(p));
    }
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?;
    for name in ["bambu-plotter.exe", "BambuPlotter.exe"] {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn text_result(v: &Value, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()}], "isError": is_error})
}

/// POST JSON в API приложения.
fn send(body: &Value) -> Result<Value, String> {
    let mut s = TcpStream::connect_timeout(&([127, 0, 0, 1], PORT).into(), Duration::from_millis(500))
        .map_err(|e| format!("приложение не запущено ({e})"))?;
    s.set_read_timeout(Some(Duration::from_secs(70))).ok();
    let data = body.to_string();
    write!(s, "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", data.len(), data)
        .map_err(|e| e.to_string())?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&resp);
    let body = text.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
    serde_json::from_str(body).map_err(|e| format!("плохой ответ: {e}"))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
