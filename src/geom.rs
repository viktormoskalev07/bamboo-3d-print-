pub type Pt = (f64, f64);
pub type Paths = Vec<Vec<Pt>>;

pub fn dist(a: Pt, b: Pt) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// [min_x, min_y, max_x, max_y]
pub fn bbox(paths: &Paths) -> Option<[f64; 4]> {
    let mut it = paths.iter().flatten();
    let first = it.next()?;
    let mut b = [first.0, first.1, first.0, first.1];
    for p in it {
        b[0] = b[0].min(p.0);
        b[1] = b[1].min(p.1);
        b[2] = b[2].max(p.0);
        b[3] = b[3].max(p.1);
    }
    Some(b)
}

pub fn map(paths: &mut Paths, f: impl Fn(Pt) -> Pt) {
    for p in paths.iter_mut().flatten() {
        *p = f(*p);
    }
}

/// Вписывает рисунок в прямоугольник (0,0)-(w,h) с сохранением пропорций.
/// `upscale=false` — только уменьшать. Рисунок центрируется.
pub fn fit(paths: &mut Paths, w: f64, h: f64, upscale: bool) -> f64 {
    let Some(b) = bbox(paths) else { return 1.0 };
    let (bw, bh) = ((b[2] - b[0]).max(1e-9), (b[3] - b[1]).max(1e-9));
    let mut s = (w / bw).min(h / bh);
    if !upscale {
        s = s.min(1.0);
    }
    let ox = (w - bw * s) / 2.0;
    let oy = (h - bh * s) / 2.0;
    map(paths, |(x, y)| ((x - b[0]) * s + ox, (y - b[1]) * s + oy));
    s
}

/// Убирает повторяющиеся точки и пустые пути.
pub fn clean(paths: Paths) -> Paths {
    paths
        .into_iter()
        .filter_map(|p| {
            let mut out: Vec<Pt> = Vec::with_capacity(p.len());
            for pt in p {
                if out.last().map_or(true, |&l| dist(l, pt) > 0.01) {
                    out.push(pt);
                }
            }
            match out.len() {
                0 => None,
                1 => Some(vec![out[0], out[0]]), // точка
                _ => Some(out),
            }
        })
        .collect()
}

/// Сохраняет порядок путей (буква за буквой), но каждый путь можно рисовать с ближнего конца.
pub fn orient(paths: Paths, start: Pt) -> Paths {
    let mut cur = start;
    paths
        .into_iter()
        .map(|mut p| {
            if dist(cur, *p.last().unwrap()) < dist(cur, p[0]) {
                p.reverse();
            }
            cur = *p.last().unwrap();
            p
        })
        .collect()
}

/// Жадная оптимизация порядка: ближайший следующий путь, с возможностью рисовать его задом наперёд.
pub fn optimize(paths: Paths, start: Pt) -> Paths {
    let mut left: Vec<Option<Vec<Pt>>> = paths.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(left.len());
    let mut cur = start;
    for _ in 0..left.len() {
        let mut best = (f64::MAX, 0usize, false);
        for (i, p) in left.iter().enumerate() {
            let Some(p) = p else { continue };
            let d0 = dist(cur, p[0]);
            if d0 < best.0 {
                best = (d0, i, false);
            }
            let d1 = dist(cur, *p.last().unwrap());
            if d1 < best.0 {
                best = (d1, i, true);
            }
        }
        let mut p = left[best.1].take().unwrap();
        if best.2 {
            p.reverse();
        }
        cur = *p.last().unwrap();
        out.push(p);
    }
    out
}

/// Обрезка ломаных по прямоугольнику [x0, y0, x1, y1] (Лианг–Барски); части снаружи выбрасываются,
/// ломаная разрывается в местах выхода. Возвращает (результат, было_ли_что_обрезано).
pub fn clip(paths: Paths, r: [f64; 4]) -> (Paths, bool) {
    let mut out = Paths::new();
    let mut cut = false;
    let eps = 1e-9;
    for p in paths {
        let mut cur: Vec<Pt> = Vec::new();
        if p.len() == 2 && dist(p[0], p[1]) < 1e-9 {
            // точка
            let q = p[0];
            if q.0 >= r[0] - eps && q.0 <= r[2] + eps && q.1 >= r[1] - eps && q.1 <= r[3] + eps {
                out.push(p);
            } else {
                cut = true;
            }
            continue;
        }
        for w in p.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let (mut t0, mut t1) = (0.0f64, 1.0f64);
            let mut inside = true;
            for (pp, qq) in [(-dx, a.0 - r[0]), (dx, r[2] - a.0), (-dy, a.1 - r[1]), (dy, r[3] - a.1)] {
                if pp.abs() < eps {
                    if qq < -eps {
                        inside = false;
                        break;
                    }
                } else {
                    let t = qq / pp;
                    if pp < 0.0 {
                        t0 = t0.max(t);
                    } else {
                        t1 = t1.min(t);
                    }
                }
            }
            if !inside || t0 > t1 + eps {
                cut = true;
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                continue;
            }
            let pa = (a.0 + dx * t0, a.1 + dy * t0);
            let pb = (a.0 + dx * t1, a.1 + dy * t1);
            if t0 > eps || t1 < 1.0 - eps {
                cut = true;
            }
            if cur.last().is_none_or(|&l| dist(l, pa) > 1e-6) {
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur = vec![pa];
            }
            cur.push(pb);
            if t1 < 1.0 - eps {
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            }
        }
        if cur.len() >= 2 {
            out.push(cur);
        }
    }
    (out, cut)
}
