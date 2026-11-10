#![windows_subsystem = "windows"]

mod app;
mod auto;
mod config;
mod connect;
mod fonts;
mod gcode;
mod hershey;
mod geom;
mod jobs;
mod output;
mod printer;
mod raster;
mod svg;
mod table;
mod text;

use eframe::egui;
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    let launch = auto::Launch::parse(std::env::args().skip(1));
    let test = launch.is_test();
    let (w, h) = launch.size.unwrap_or((1480.0, 920.0));
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(if test { "Bambu Plotter (тест)" } else { "Bambu Plotter" })
        .with_inner_size([w, h])
        .with_min_inner_size([900.0, 600.0])
        .with_drag_and_drop(true)
        .with_icon(Arc::new(icon()));
    if launch.offscreen {
        // окно за пределами экрана: рендер идёт, пользователю не мешает
        viewport = viewport.with_position([-20000.0, -20000.0]).with_active(false).with_taskbar(false);
    } else if test {
        viewport = viewport.with_active(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        persist_window: !test,
        ..Default::default()
    };
    // у тестовых запусков отдельное хранилище состояния
    let name = launch.storage_name();
    eframe::run_native(&name, options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, launch)))))
}

/// Иконка окна: зелёный скруглённый квадрат с белым пером.
fn icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let r = 14.0;
            let cx = fx.clamp(r, n as f32 - r);
            let cy = fy.clamp(r, n as f32 - r);
            let inside = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() <= r;
            if !inside {
                continue;
            }
            // диагональная линия пера
            let d = ((fx - (n as f32 - fy)).abs()) / std::f32::consts::SQRT_2;
            let on_line = d < 3.5 && fx > 14.0 && fx < 50.0;
            let dot = ((fx - 15.0).powi(2) + (fy - 49.0).powi(2)).sqrt() < 5.0;
            let c: [u8; 4] = if on_line || dot { [255, 255, 255, 255] } else { [0, 174, 66, 255] };
            rgba[(y * n + x) * 4..][..4].copy_from_slice(&c);
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}
