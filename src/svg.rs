use crate::geom::{dist, Paths, Pt};
use anyhow::{Context, Result};
use std::path::Path;
use usvg::tiny_skia_path::PathSegment;

/// Читает SVG и возвращает контуры в мм (1px = 25.4/96 мм), ось Y вниз.
pub fn load(path: &Path) -> Result<Paths> {
    let data = std::fs::read(path).with_context(|| format!("не удалось прочитать {}", path.display()))?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).context("не удалось разобрать SVG")?;
    let mut out = Paths::new();
    walk(tree.root(), &mut out);
    let k = 25.4 / 96.0;
    crate::geom::map(&mut out, |(x, y)| (x * k, y * k));
    Ok(out)
}

fn walk(g: &usvg::Group, out: &mut Paths) {
    for node in g.children() {
        match node {
            usvg::Node::Group(g) => walk(g, out),
            usvg::Node::Path(p) => add_path(p, out),
            usvg::Node::Text(t) => walk(t.flattened(), out),
            _ => {}
        }
    }
}

fn add_path(p: &usvg::Path, out: &mut Paths) {
    let ts = p.abs_transform();
    let tr = |x: f32, y: f32| -> Pt {
        let (x, y) = (x as f64, y as f64);
        (
            ts.sx as f64 * x + ts.kx as f64 * y + ts.tx as f64,
            ts.ky as f64 * x + ts.sy as f64 * y + ts.ty as f64,
        )
    };
    let mut cur: Vec<Pt> = Vec::new();
    let mut start: Pt = (0.0, 0.0);
    let mut last: Pt = (0.0, 0.0);
    let steps = |len: f64| ((len / 1.0).ceil() as usize).clamp(2, 64); // ~1px на сегмент
    for seg in p.data().segments() {
        match seg {
            PathSegment::MoveTo(pt) => {
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                last = tr(pt.x, pt.y);
                start = last;
                cur.push(last);
            }
            PathSegment::LineTo(pt) => {
                last = tr(pt.x, pt.y);
                cur.push(last);
            }
            PathSegment::QuadTo(p1, p2) => {
                let (a, b, c) = (last, tr(p1.x, p1.y), tr(p2.x, p2.y));
                let n = steps(dist(a, b) + dist(b, c));
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let u = 1.0 - t;
                    cur.push((
                        u * u * a.0 + 2.0 * u * t * b.0 + t * t * c.0,
                        u * u * a.1 + 2.0 * u * t * b.1 + t * t * c.1,
                    ));
                }
                last = c;
            }
            PathSegment::CubicTo(p1, p2, p3) => {
                let (a, b, c, d) = (last, tr(p1.x, p1.y), tr(p2.x, p2.y), tr(p3.x, p3.y));
                let n = steps(dist(a, b) + dist(b, c) + dist(c, d));
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let u = 1.0 - t;
                    cur.push((
                        u * u * u * a.0 + 3.0 * u * u * t * b.0 + 3.0 * u * t * t * c.0 + t * t * t * d.0,
                        u * u * u * a.1 + 3.0 * u * u * t * b.1 + 3.0 * u * t * t * c.1 + t * t * t * d.1,
                    ));
                }
                last = d;
            }
            PathSegment::Close => {
                cur.push(start);
                last = start;
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            }
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
}
