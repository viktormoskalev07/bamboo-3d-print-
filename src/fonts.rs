//! Список установленных в системе шрифтов (с именами и поддержкой кириллицы).

use ttf_parser::{name_id, Face};

#[derive(Clone)]
pub struct FontEntry {
    pub name: String,
    pub path: String,
    pub cyrillic: bool,
}

pub fn scan() -> Vec<FontEntry> {
    let mut dirs = vec![std::path::PathBuf::from(r"C:\Windows\Fonts")];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        dirs.push(std::path::Path::new(&local).join(r"Microsoft\Windows\Fonts"));
    }
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
            if ext != "ttf" && ext != "otf" {
                continue;
            }
            // очень большие (CJK) шрифты пропускаем — долго и не нужны
            if e.metadata().map(|m| m.len() > 12_000_000).unwrap_or(true) {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else { continue };
            let Ok(face) = Face::parse(&data, 0) else { continue };
            let name = face
                .names()
                .into_iter()
                .filter(|n| n.name_id == name_id::FULL_NAME)
                .find_map(|n| n.to_string())
                .unwrap_or_else(|| p.file_stem().unwrap_or_default().to_string_lossy().to_string());
            out.push(FontEntry { name, path: p.display().to_string(), cyrillic: face.glyph_index('Ж').is_some() });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.name == b.name);
    out
}
