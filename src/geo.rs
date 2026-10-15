//! Web Mercator, measuring, GeoJSON shapes, and drawing marks into the map image.

use image::{Rgba, RgbaImage};
use serde_json::Value;
use std::f64::consts::PI;

const R_MERC: f64 = 6378137.0; // Web Mercator sphere
const R_EARTH: f64 = 6371008.8; // mean radius, for measuring

pub fn merc(lon: f64, lat: f64) -> (f64, f64) {
    let lat = lat.clamp(-85.0511, 85.0511).to_radians();
    (R_MERC * lon.to_radians(), R_MERC * (PI / 4.0 + lat / 2.0).tan().ln())
}

pub fn unmerc(x: f64, y: f64) -> (f64, f64) {
    ((x / R_MERC).to_degrees(), (2.0 * (y / R_MERC).exp().atan() - PI / 2.0).to_degrees())
}

/// Where a map image sits in Web Mercator: centre, metres per image pixel, image size.
#[derive(Clone, Copy)]
pub struct View {
    pub cx: f64,
    pub cy: f64,
    pub res: f64,
    pub w: f64,
    pub h: f64,
}

impl View {
    /// view_map fits the map's own extent into the image, centred (checked against a rendered
    /// selection on 900x500 and 500x700 images), so the extent gives the image's geometry.
    pub fn from_extent([x0, y0, x1, y1]: [f64; 4], w: f64, h: f64) -> View {
        let (a, b) = (merc(x0, y0), merc(x1, y1));
        View { cx: (a.0 + b.0) / 2.0, cy: (a.1 + b.1) / 2.0, res: ((b.0 - a.0) / w).max((b.1 - a.1) / h), w, h }
    }
    pub fn px(&self, lon: f64, lat: f64) -> (f64, f64) {
        let (x, y) = merc(lon, lat);
        ((x - self.cx) / self.res + self.w / 2.0, (self.cy - y) / self.res + self.h / 2.0)
    }
    pub fn geo(&self, px: f64, py: f64) -> (f64, f64) {
        unmerc(self.cx + (px - self.w / 2.0) * self.res, self.cy - (py - self.h / 2.0) * self.res)
    }
    /// Ground metres per image pixel at the centre.
    pub fn ground_res(&self) -> f64 {
        self.res * self.geo(self.w / 2.0, self.h / 2.0).1.to_radians().cos()
    }
}

/// Great-circle length of a path of lon, lat points, in metres.
pub fn path_len(pts: &[(f64, f64)]) -> f64 {
    pts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let (p1, p2) = (a.1.to_radians(), b.1.to_radians());
            let h = ((p2 - p1) / 2.0).sin().powi(2) + p1.cos() * p2.cos() * ((b.0 - a.0).to_radians() / 2.0).sin().powi(2);
            2.0 * R_EARTH * h.sqrt().asin()
        })
        .sum()
}

/// Area of a lon, lat ring on the sphere, in square metres (Chamberlain & Duquette).
pub fn ring_area(pts: &[(f64, f64)]) -> f64 {
    let n = pts.len();
    let sum: f64 = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            (b.0 - a.0).to_radians() * (2.0 + a.1.to_radians().sin() + b.1.to_radians().sin())
        })
        .sum();
    (sum * R_EARTH * R_EARTH / 2.0).abs()
}

pub fn fmt_len(m: f64) -> String {
    if m < 1000.0 { format!("{m:.1} m") } else { format!("{:.3} km", m / 1000.0) }
}

pub fn fmt_area(m2: f64) -> String {
    if m2 < 1e6 { format!("{m2:.0} m²") } else { format!("{:.3} km²", m2 / 1e6) }
}

/// A round length (1, 2 or 5 x 10^n metres) no longer than `max` metres, and its label.
pub fn nice_len(max: f64) -> (f64, String) {
    let p = 10f64.powf(max.log10().floor());
    let m = [5.0, 2.0, 1.0].into_iter().map(|k| k * p).find(|&m| m <= max).unwrap_or(p);
    let label = if m >= 1000.0 { format!("{} km", m / 1000.0) } else { format!("{m} m") };
    (m, label)
}

/// A shape to draw: one point, or a path (closed for polygon rings).
pub enum Part {
    Point(f64, f64),
    Path(Vec<(f64, f64)>, bool),
}

/// Every part of every feature in a GeoJSON FeatureCollection (lon, lat).
pub fn parts(fc: &Value) -> Vec<Part> {
    fn pt(v: &Value) -> Option<(f64, f64)> {
        Some((v[0].as_f64()?, v[1].as_f64()?))
    }
    fn line(v: &Value) -> Vec<(f64, f64)> {
        v.as_array().into_iter().flatten().filter_map(pt).collect()
    }
    fn add(g: &Value, out: &mut Vec<Part>) {
        let c = &g["coordinates"];
        let each = || c.as_array().into_iter().flatten();
        match g["type"].as_str() {
            Some("Point") => out.extend(pt(c).map(|(x, y)| Part::Point(x, y))),
            Some("MultiPoint") => out.extend(each().filter_map(pt).map(|(x, y)| Part::Point(x, y))),
            Some("LineString") => out.push(Part::Path(line(c), false)),
            Some("MultiLineString") => out.extend(each().map(|l| Part::Path(line(l), false))),
            Some("Polygon") => out.extend(each().map(|r| Part::Path(line(r), true))),
            Some("MultiPolygon") => out.extend(each().flat_map(|p| p.as_array().cloned().unwrap_or_default()).map(|r| Part::Path(line(&r), true))),
            Some("GeometryCollection") => g["geometries"].as_array().into_iter().flatten().for_each(|g| add(g, out)),
            _ => {}
        }
    }
    let mut out = vec![];
    for f in fc["features"].as_array().into_iter().flatten() {
        add(&f["geometry"], &mut out);
    }
    out
}

pub fn bbox(parts: &[Part]) -> Option<[f64; 4]> {
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut grow = |(x, y): (f64, f64)| b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    for p in parts {
        match p {
            Part::Point(x, y) => grow((*x, *y)),
            Part::Path(pts, _) => pts.iter().copied().for_each(&mut grow),
        }
    }
    (b[0] <= b[2]).then_some(b)
}

const HALO: Rgba<u8> = Rgba([25, 25, 25, 255]);

/// Drawing into the map image. Marks go into the pixels, never into the terminal cells: in
/// Ghostty, Kitty and the other image terminals a character written over the image breaks it.
pub struct Canvas<'a> {
    pub im: &'a mut RgbaImage,
    pub v: View,
}

impl Canvas<'_> {
    fn dot(&mut self, (x, y): (f64, f64), r: f64, c: Rgba<u8>) {
        let (w, h) = (self.im.width() as i64, self.im.height() as i64);
        let ri = r.ceil() as i64;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                let (px, py) = (x as i64 + dx, y as i64 + dy);
                if ((dx * dx + dy * dy) as f64) <= r * r && px >= 0 && py >= 0 && px < w && py < h {
                    self.im.put_pixel(px as u32, py as u32, c);
                }
            }
        }
    }

    fn seg(&mut self, a: (f64, f64), b: (f64, f64), r: f64, c: Rgba<u8>) {
        let (w, h) = (self.v.w, self.v.h);
        if (a.0 < 0.0 && b.0 < 0.0) || (a.1 < 0.0 && b.1 < 0.0) || (a.0 > w && b.0 > w) || (a.1 > h && b.1 > h) {
            return; // off the image
        }
        let n = (b.0 - a.0).hypot(b.1 - a.1).ceil().clamp(1.0, 20_000.0);
        for i in 0..=n as usize {
            let t = i as f64 / n;
            self.dot((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t), r, c);
        }
    }

    fn path(&mut self, pts: &[(f64, f64)], closed: bool, r: f64, c: Rgba<u8>) {
        let px: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| self.v.px(x, y)).collect();
        for w in px.windows(2) {
            self.seg(w[0], w[1], r, c);
        }
        if closed && px.len() > 2 {
            self.seg(px[px.len() - 1], px[0], r, c);
        }
    }

    /// Fade the whole image towards the background, so what is drawn next stands out.
    pub fn dim(&mut self, keep: f32) {
        for p in self.im.pixels_mut() {
            for k in 0..3 {
                p[k] = (p[k] as f32 * keep + 30.0 * (1.0 - keep)) as u8;
            }
        }
    }

    /// A layer's shapes in its accent colour, on a dark halo.
    pub fn highlight(&mut self, parts: &[Part], c: [u8; 3]) {
        let c = Rgba([c[0], c[1], c[2], 255]);
        for pass in [(3.0, HALO, 6.0), (1.5, c, 4.0)] {
            for p in parts {
                match p {
                    Part::Point(x, y) => {
                        let at = self.v.px(*x, *y);
                        self.dot(at, pass.2, pass.1)
                    }
                    Part::Path(pts, closed) => self.path(pts, *closed, pass.0, pass.1),
                }
            }
        }
    }

    /// A sketch being drawn: its lines, then its points.
    pub fn sketch(&mut self, pts: &[(f64, f64)], closed: bool) {
        let yellow = Rgba([255, 210, 0, 255]);
        self.path(pts, closed, 1.5, yellow);
        for &(x, y) in pts {
            let at = self.v.px(x, y);
            self.dot(at, 6.0, HALO);
            self.dot(at, 4.0, yellow);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_and_measures() {
        let (x, y) = merc(114.17, 22.3);
        let (lon, lat) = unmerc(x, y);
        assert!((lon - 114.17).abs() < 1e-9 && (lat - 22.3).abs() < 1e-9);
        let v = View::from_extent([-10.0, -10.0, 10.0, 10.0], 400.0, 200.0);
        let (px, py) = v.px(3.0, 4.0);
        let (lon, lat) = v.geo(px, py);
        assert!((lon - 3.0).abs() < 1e-9 && (lat - 4.0).abs() < 1e-9);
        // one degree along the equator, and a 1° x 1° cell at the equator
        assert!((path_len(&[(0.0, 0.0), (1.0, 0.0)]) - 111_195.0).abs() < 1.0);
        let a = ring_area(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
        assert!((a / 1e6 - 12_363.7).abs() < 1.0, "{a}");
        assert_eq!(nice_len(730_000.0), (500_000.0, "500 km".into()));
        assert_eq!(nice_len(23.0), (20.0, "20 m".into()));
    }

    #[test]
    fn geojson_parts() {
        let fc: Value = serde_json::from_str(
            r#"{"features":[{"geometry":{"type":"MultiPolygon","coordinates":[[[[0,0],[2,0],[2,1],[0,0]]],[[[5,5],[6,5],[6,6],[5,5]]]]}},
                {"geometry":{"type":"Point","coordinates":[-3,7]}}]}"#,
        )
        .unwrap();
        let p = parts(&fc);
        assert_eq!(p.len(), 3);
        assert_eq!(bbox(&p), Some([-3.0, 0.0, 6.0, 7.0]));
    }
}
