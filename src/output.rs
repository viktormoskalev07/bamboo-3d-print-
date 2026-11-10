#![allow(dead_code)]
use crate::config::{Config, BED_H, BED_W};
use crate::gcode::Stroke;
use anyhow::Result;
use std::io::Write;
use std::path::Path;

// Шаблоны метаданных — из настоящего .gcode.3mf, нарезанного Bambu Studio для A1.
// Bambu Connect без полного набора полей падает (белый экран).
const T_CONTENT_TYPES: &str = include_str!("../templates/a1/content_types.xml");
const T_RELS: &str = include_str!("../templates/a1/rels.xml");
const T_MODEL: &str = include_str!("../templates/a1/3dmodel.model");
const T_MODEL_SETTINGS: &str = include_str!("../templates/a1/model_settings.config");
const T_MODEL_SETTINGS_RELS: &str = include_str!("../templates/a1/model_settings_rels.xml");
const T_SLICE_INFO: &str = include_str!("../templates/a1/slice_info.config");
const T_PROJECT_SETTINGS: &str = include_str!("../templates/a1/project_settings.config");
const T_FILAMENT_SEQ: &str = include_str!("../templates/a1/filament_sequence.json");
const T_CUT_INFO: &str = include_str!("../templates/a1/cut_information.xml");

/// Пакует G-code в .gcode.3mf в формате Bambu Studio (A1): печать с SD-карты, через Bambu Connect.
/// `bbox` — габариты рисунка в координатах стола [x0, y0, x1, y1].
pub fn write_3mf(path: &Path, gcode: &str, thumb_png: &[u8], secs: f64) -> Result<()> {
    write_3mf_ex(path, gcode, thumb_png, secs, [0.0, 0.0, 0.0, 0.0])
}

pub fn write_3mf_ex(path: &Path, gcode: &str, thumb_png: &[u8], secs: f64, bbox: [f64; 4]) -> Result<()> {
    let f = std::fs::File::create(path)?;
    let mut z = zip::ZipWriter::new(f);
    let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let md5 = format!("{:X}", md5::compute(gcode.as_bytes()));
    let small = resize_png(thumb_png, 128).unwrap_or_else(|_| thumb_png.to_vec());

    let today = today();
    let model = T_MODEL
        .replace("BambuStudio-02.08.02.61", "BambuStudio-02.08.02.61")
        .replace(">2026-08-23<", &format!(">{today}<"));

    // slice_info: время, вес 0, без чужого объекта/предупреждений
    let mut slice = String::new();
    for line in T_SLICE_INFO.lines() {
        let t = line.trim_start();
        if t.starts_with("<warning") {
            continue;
        }
        let l = if t.starts_with(r#"<metadata key="prediction""#) {
            format!(r#"    <metadata key="prediction" value="{:.0}"/>"#, secs)
        } else if t.starts_with(r#"<metadata key="weight""#) {
            r#"    <metadata key="weight" value="0.00"/>"#.to_string()
        } else if t.starts_with(r#"<metadata key="first_layer_time""#) {
            format!(r#"    <metadata key="first_layer_time" value="{:.6}"/>"#, secs)
        } else if t.starts_with("<object ") {
            r#"    <object identify_id="534" name="plot" skipped="false" />"#.to_string()
        } else if t.starts_with("<filament ") {
            line.replace(r#"used_g="0.01""#, r#"used_g="0.00""#)
        } else {
            line.to_string()
        };
        slice.push_str(&l);
        slice.push('\n');
    }

    let plate_json = serde_json::json!({
        "bbox_all": bbox,
        "bbox_objects": [{"area": ((bbox[2]-bbox[0]) * (bbox[3]-bbox[1])).max(0.0), "bbox": bbox, "id": 589,
                          "layer_height": 0.2, "name": "plot"}],
        "bed_type": "textured_plate",
        "filament_colors": ["#FFFFFF"],
        "filament_ids": [0],
        "first_extruder": 0,
        "first_layer_time": secs,
        "is_seq_print": false,
        "nozzle_diameter": 0.4,
        "version": 2
    })
    .to_string();

    let files: Vec<(&str, Vec<u8>)> = vec![
        ("[Content_Types].xml", T_CONTENT_TYPES.as_bytes().to_vec()),
        ("_rels/.rels", T_RELS.as_bytes().to_vec()),
        ("3D/3dmodel.model", model.into_bytes()),
        ("Metadata/_rels/model_settings.config.rels", T_MODEL_SETTINGS_RELS.as_bytes().to_vec()),
        ("Metadata/model_settings.config", T_MODEL_SETTINGS.as_bytes().to_vec()),
        ("Metadata/slice_info.config", slice.into_bytes()),
        ("Metadata/project_settings.config", T_PROJECT_SETTINGS.as_bytes().to_vec()),
        ("Metadata/filament_sequence.json", T_FILAMENT_SEQ.as_bytes().to_vec()),
        ("Metadata/cut_information.xml", T_CUT_INFO.as_bytes().to_vec()),
        ("Metadata/plate_1.json", plate_json.into_bytes()),
        ("Metadata/plate_1.gcode", gcode.as_bytes().to_vec()),
        ("Metadata/plate_1.gcode.md5", md5.into_bytes()),
        ("Metadata/plate_1.png", thumb_png.to_vec()),
        ("Metadata/plate_1_small.png", small),
        ("Metadata/plate_no_light_1.png", thumb_png.to_vec()),
        ("Metadata/top_1.png", thumb_png.to_vec()),
        ("Metadata/pick_1.png", thumb_png.to_vec()),
    ];
    for (name, data) in files {
        z.start_file(name, opt)?;
        z.write_all(&data)?;
    }
    z.finish()?;
    Ok(())
}

fn today() -> String {
    // дата без внешних библиотек (UTC)
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86400) as i64;
    let (mut y, mut d) = (1970i64, days);
    loop {
        let len = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 366 } else { 365 };
        if d < len {
            break;
        }
        d -= len;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let months = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    while d >= months[m] {
        d -= months[m];
        m += 1;
    }
    format!("{y:04}-{:02}-{:02}", m + 1, d + 1)
}

/// Уменьшение PNG (ближайший сосед) для маленькой миниатюры.
fn resize_png(data: &[u8], size: u32) -> Result<Vec<u8>> {
    let dec = png::Decoder::new(std::io::Cursor::new(data));
    let mut r = dec.read_info()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = info.line_size / w;
    let mut out = vec![0u8; (size * size) as usize * 4];
    for y in 0..size as usize {
        for x in 0..size as usize {
            let sx = x * w / size as usize;
            let sy = y * h / size as usize;
            let s = &buf[sy * info.line_size + sx * ch..][..ch];
            let px = match ch {
                4 => [s[0], s[1], s[2], s[3]],
                3 => [s[0], s[1], s[2], 255],
                _ => [s[0], s[0], s[0], 255],
            };
            out[(y * size as usize + x) * 4..][..4].copy_from_slice(&px);
        }
    }
    let mut o = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut o, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&out)?;
    }
    Ok(o)
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
 <Default Extension="png" ContentType="image/png"/>
 <Default Extension="gcode" ContentType="text/x.gcode"/>
</Types>
"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>
"#;

const MODEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <metadata name="Application">BambuPlotter</metadata>
 <resources/>
 <build/>
</model>
"#;

const MODEL_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <plate>
    <metadata key="plater_id" value="1"/>
    <metadata key="plater_name" value=""/>
    <metadata key="locked" value="false"/>
    <metadata key="gcode_file" value="Metadata/plate_1.gcode"/>
    <metadata key="thumbnail_file" value="Metadata/plate_1.png"/>
  </plate>
</config>
"#;

const PROJECT_SETTINGS: &str = r##"{
    "printer_model": "Bambu Lab A1",
    "printer_settings_id": "Bambu Lab A1 0.4 nozzle",
    "inherits": "Bambu Lab A1 0.4 nozzle",
    "printer_variant": "0.4",
    "nozzle_diameter": ["0.4"],
    "printable_area": ["0x0", "256x0", "256x256", "0x256"],
    "printable_height": "256",
    "curr_bed_type": "Textured PEI Plate",
    "filament_type": ["PLA"],
    "filament_colour": ["#000000"],
    "filament_settings_id": ["Bambu PLA Basic @BBL A1"],
    "filament_ids": ["GFA00"],
    "print_settings_id": "0.20mm Standard @BBL A1",
    "layer_height": "0.2",
    "version": "01.10.00.00"
}
"##;

const SLICE_INFO: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <header>
    <header_item key="X-BBL-Client-Type" value="slicer"/>
    <header_item key="X-BBL-Client-Version" value="01.10.00.00"/>
  </header>
  <plate>
    <metadata key="index" value="1"/>
    <metadata key="printer_model_id" value="N2S"/>
    <metadata key="nozzle_diameters" value="0.4"/>
    <metadata key="timelapse_type" value="0"/>
    <metadata key="prediction" value="{SECS}"/>
    <metadata key="weight" value="0"/>
    <metadata key="outside" value="false"/>
    <metadata key="support_used" value="false"/>
    <metadata key="label_object_enabled" value="false"/>
    <filament id="1" tray_info_idx="GFA00" type="PLA" color="#000000" used_m="0.00" used_g="0.00"/>
  </plate>
</config>
"##;

/// Миниатюра листа для экрана принтера.
pub fn thumbnail(strokes: &[Stroke], cfg: &Config) -> Result<Vec<u8>> {
    let (ox, oy) = cfg.paper_origin();
    let (pw, ph) = (cfg.paper.width, cfg.paper.height);
    let size = 512usize;
    let k = (size as f64 - 8.0) / pw.max(ph);
    let (w, h) = (size, size);
    let offx = (size as f64 - pw * k) / 2.0;
    let offy = (size as f64 - ph * k) / 2.0;
    let mut img = vec![0u8; w * h * 4];
    // лист
    for y in offy as usize..(offy + ph * k) as usize {
        for x in offx as usize..(offx + pw * k) as usize {
            img[(y * w + x) * 4..][..4].copy_from_slice(&[255, 255, 255, 255]);
        }
    }
    let to_px = |(x, y): (f64, f64)| (offx + (x - ox) * k, offy + (oy + ph - y) * k);
    let mut plot = |x: f64, y: f64| {
        for (dx, dy) in [(0i32, 0i32), (1, 0), (0, 1), (1, 1)] {
            let (px, py) = (x as i32 + dx, y as i32 + dy);
            if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                img[(py as usize * w + px as usize) * 4..][..4].copy_from_slice(&[20, 40, 140, 255]);
            }
        }
    };
    for s in strokes {
        for seg in s.pts.windows(2) {
            let (a, b) = (to_px(seg[0]), to_px(seg[1]));
            let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize).max(1);
            for i in 0..=n {
                let t = i as f64 / n as f64;
                plot(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            }
        }
    }
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&img)?;
    }
    Ok(buf)
}

/// SVG-превью: стол, лист, линии пера (синие) и холостые перемещения (красный пунктир).
pub fn preview_svg(strokes: &[Stroke], cfg: &Config, title: &str, info: &str) -> String {
    let (ox, oy) = cfg.paper_origin();
    let p = &cfg.paper;
    let pen = &cfg.pen;
    let mut s = String::new();
    s += &format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-10 -24 {} {}" width="900" style="background:#f4f4f4;font-family:sans-serif">
<text x="0" y="-12" font-size="7">{}</text><text x="0" y="-4" font-size="4.5" fill="#555">{}</text>
<g transform="translate(0,{}) scale(1,-1)">
<rect x="0" y="0" width="{BED_W}" height="{BED_H}" fill="#2b2b2b"/>
<rect x="{}" y="{}" width="{BED_W}" height="{BED_H}" fill="none" stroke="#4caf50" stroke-width="0.6" stroke-dasharray="3 2"/>
<rect x="{ox}" y="{oy}" width="{}" height="{}" fill="#fff"/>
<rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="#ccc" stroke-width="0.3" stroke-dasharray="1 1"/>
"##,
        BED_W + 20.0,
        BED_H + 34.0,
        xml(title),
        xml(info),
        BED_H,
        pen.offset_x,
        pen.offset_y,
        p.width,
        p.height,
        ox + p.margin,
        oy + p.margin,
        p.width - 2.0 * p.margin,
        p.height - 2.0 * p.margin,
    );
    // холостые перемещения
    let mut travel = String::new();
    let mut last: Option<(f64, f64)> = None;
    for st in strokes {
        if let Some(l) = last {
            let f = st.pts[0];
            if crate::geom::dist(l, f) > 0.05 {
                travel += &format!("M{:.2} {:.2}L{:.2} {:.2}", l.0, l.1, f.0, f.1);
            }
        }
        last = st.pts.last().copied();
    }
    s += &format!(r##"<path d="{travel}" fill="none" stroke="#e53935" stroke-width="0.15" stroke-dasharray="0.8 0.8" opacity="0.6"/>"##);
    let mut d = String::new();
    for st in strokes {
        d += &format!("M{:.2} {:.2}", st.pts[0].0, st.pts[0].1);
        for p in &st.pts[1..] {
            d += &format!("L{:.2} {:.2}", p.0, p.1);
        }
    }
    s += &format!(
        r##"<path d="{d}" fill="none" stroke="#1a3a9c" stroke-width="0.35" stroke-linecap="round" stroke-linejoin="round"/>"##
    );
    s += "</g></svg>\n";
    s
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
