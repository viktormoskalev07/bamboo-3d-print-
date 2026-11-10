use crate::geom::Paths;
use crate::text::Font;
use anyhow::{bail, Context, Result};
use std::path::Path;

pub fn read_csv(path: &Path) -> Result<Vec<Vec<String>>> {
    let raw = std::fs::read(path).with_context(|| format!("не удалось прочитать {}", path.display()))?;
    // UTF-8 (с BOM или без); если не UTF-8 — пробуем Windows-1251 (Excel любит его)
    let text = match String::from_utf8(raw.clone()) {
        Ok(s) => s.trim_start_matches('\u{feff}').to_string(),
        Err(_) => raw.iter().map(|&b| cp1251(b)).collect(),
    };
    parse(&text)
}

/// Разбор CSV/TSV из строки (разделитель определяется сам; подходит и для вставки из Excel).
pub fn parse(text: &str) -> Result<Vec<Vec<String>>> {
    let first = text.lines().next().unwrap_or("");
    let delim = [b';', b'\t', b',', b'|']
        .into_iter()
        .max_by_key(|d| first.bytes().filter(|b| b == d).count())
        .unwrap();
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for r in rdr.records() {
        let r = r?;
        rows.push(r.iter().map(|s| s.trim().to_string()).collect());
    }
    if rows.is_empty() {
        bail!("таблица пустая");
    }
    Ok(rows)
}

fn cp1251(b: u8) -> char {
    match b {
        0x00..=0x7F => b as char,
        0xC0..=0xFF => char::from_u32(0x0410 + (b as u32 - 0xC0)).unwrap(),
        0xA8 => 'Ё',
        0xB8 => 'ё',
        0xB9 => '№',
        _ => ' ',
    }
}

fn is_number(s: &str) -> bool {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    !t.is_empty() && t.replace(',', ".").trim_end_matches('%').parse::<f64>().is_ok()
}

/// Таблица с сеткой. Первая строка — заголовок, повторяется на каждой странице.
pub fn layout(
    font: &Font,
    rows: &[Vec<String>],
    size: f64,
    line_spacing: f64,
    area: (f64, f64),
    has_header: bool,
) -> Vec<Paths> {
    let (aw, ah) = area;
    if rows.is_empty() {
        return vec![];
    }
    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
    let col_widths = |size: f64| -> Vec<f64> {
        let pad = size * 0.5;
        (0..ncols)
            .map(|c| {
                rows.iter().map(|r| r.get(c).map_or(0.0, |s| font.width(s, size))).fold(0.0, f64::max) + 2.0 * pad
            })
            .collect()
    };
    // если не влезает по ширине — уменьшаем шрифт
    let mut size = size;
    let total: f64 = col_widths(size).iter().sum();
    if total > aw {
        size *= aw / total;
    }
    let widths = col_widths(size);
    let pad = size * 0.5;
    let asc = font.ascender(size);
    let desc = font.descender(size);
    let rh = (size * crate::text::LINE_K * line_spacing).max(asc + desc) + pad;
    let table_w: f64 = widths.iter().sum();

    let (header, body): (Option<&Vec<String>>, &[Vec<String>]) =
        if has_header { (Some(&rows[0]), &rows[1..]) } else { (None, rows) };
    let fit_rows = ((ah / rh).floor() as usize).max(1);
    let per_page = if has_header { fit_rows.saturating_sub(1).max(1) } else { fit_rows };
    let chunks: Vec<&[Vec<String>]> = if body.is_empty() { vec![&[]] } else { body.chunks(per_page).collect() };

    chunks
        .into_iter()
        .map(|chunk| {
            let mut out = Paths::new();
            let page_rows: Vec<&Vec<String>> = header.into_iter().chain(chunk.iter()).collect();
            let n = page_rows.len();
            // горизонтальные линии
            for i in 0..=n {
                let y = i as f64 * rh;
                out.push(vec![(0.0, y), (table_w, y)]);
            }
            // двойная линия под заголовком
            if has_header && n > 1 {
                out.push(vec![(0.0, rh + 0.6), (table_w, rh + 0.6)]);
            }
            // вертикальные линии
            let mut x = 0.0;
            for i in 0..=ncols {
                out.push(vec![(x, 0.0), (x, n as f64 * rh)]);
                if i < ncols {
                    x += widths[i];
                }
            }
            // текст
            for (ri, row) in page_rows.iter().enumerate() {
                let top = ri as f64 * rh;
                let baseline = top + (rh - (asc + desc)) / 2.0 + asc;
                let mut x = 0.0;
                for (ci, w) in widths.iter().enumerate() {
                    if let Some(s) = row.get(ci) {
                        let tx = if (ri > 0 || !has_header) && is_number(s) { x + w - pad - font.width(s, size) } else { x + pad };
                        font.draw(s, tx, baseline, size, &mut out);
                    }
                    x += w;
                }
            }
            out
        })
        .collect()
}
