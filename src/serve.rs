//! `--serve DIR`: run without a terminal, for a Claude Code pane (or any other front end).
//!
//! Each frame goes to `DIR/frame.json` (written whole, then renamed into place), then its number
//! to `DIR/gen`. A frame holds the screen as cells in the layout of the pane's `Raster` element
//! (base64 of little-endian u32 triplets: code point, foreground, background), the same screen as
//! plain text lines, and where the map sits with the PNG that fills it (`DIR/map-N.png`).
//!
//! Input arrives as files in `DIR/in/`, read in name order and deleted, each a JSON object or
//! an array of them:
//!   {"key": "j"} {"key": "S-Left"} {"key": "Enter"}     the key map's own spelling (keys::spec)
//!   {"mouse": "down"|"up"|"move"|"scroll", "x": col, "y": row, "button": "left"|"right",
//!    "dir": "up"|"down", "shift": true}
//!   {"size": [cols, rows]}   {"quit": true}

use crate::App;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::{Color, Modifier};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Serve {
    pub dir: PathBuf,
    map_seq: u64,
    pub map_file: Option<PathBuf>,
}

impl Serve {
    /// Save the map image as the next `map-N.png` (renamed into place), dropping old ones.
    pub fn save_map(&mut self, img: &image::DynamicImage) {
        self.map_seq += 1;
        let path = self.dir.join(format!("map-{}.png", self.map_seq));
        let tmp = self.dir.join("map.tmp.png");
        if img.save(&tmp).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            self.map_file = Some(path);
        }
        if self.map_seq > 4 {
            let _ = std::fs::remove_file(self.dir.join(format!("map-{}.png", self.map_seq - 4)));
        }
    }
}

/// Image pixels per cell for a screen `cols` wide: as fine as it can be while the map (all but
/// the 36-column Layers pane) stays within view_map's 2048-pixel limit, so the image is never cut
/// and a cell maps to an exact block of pixels.
fn cell_size(cols: u16) -> (u16, u16) {
    let w = (2048 / cols.saturating_sub(36).max(1)).clamp(6, 16);
    (w, w * 2)
}

#[allow(deprecated)] // from_fontsize: there is no terminal to ask, the size is ours to choose
fn picker(cols: u16) -> ratatui_image::picker::Picker {
    let mut p = ratatui_image::picker::Picker::from_fontsize(cell_size(cols).into());
    p.set_protocol_type(ratatui_image::picker::ProtocolType::Halfblocks);
    p
}

pub fn start(dir: &Path) -> std::io::Result<(Serve, ratatui_image::picker::Picker)> {
    // Input already waiting is read, not dropped: the front end may send the pane's size before
    // this has started (the Claude Code mod clears the folder before starting it).
    std::fs::create_dir_all(dir.join("in"))?;
    Ok((Serve { dir: dir.to_path_buf(), map_seq: 0, map_file: None }, picker(120)))
}

fn new_term(cols: u16, rows: u16) -> Terminal<TestBackend> {
    let Ok(t) = Terminal::new(TestBackend::new(cols, rows));
    t
}

pub fn run(app: &mut App) -> std::io::Result<()> {
    let dir = app.serve.as_ref().map(|s| s.dir.clone()).expect("serve mode");
    let (mut cols, mut rows) = (120u16, 36u16);
    let mut term = new_term(cols, rows);
    let (mut last, mut frame_no) = (String::new(), 0u64);
    // Draw only when something happened: input, a finished background job, a new half-block
    // image; and once a second regardless, for anything else that changes on its own.
    let mut redraw = true;
    let mut drawn = std::time::Instant::now();
    while !app.quit {
        let mut inbox: Vec<PathBuf> = std::fs::read_dir(dir.join("in"))?.flatten().map(|e| e.path()).collect();
        inbox.sort();
        redraw |= !inbox.is_empty();
        for path in inbox {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let _ = std::fs::remove_file(&path);
            let v: Value = serde_json::from_str(&text).unwrap_or_default();
            let events = if let Value::Array(a) = v { a } else { vec![v] };
            for ev in events {
                if let Some([c, r]) = ev["size"].as_array().map(|a| [a[0].as_u64(), a.get(1).and_then(Value::as_u64)]) {
                    let (c, r) = (c.unwrap_or(120).clamp(40, 512) as u16, r.unwrap_or(36).clamp(12, 256) as u16);
                    if (c, r) != (cols, rows) {
                        (cols, rows) = (c, r);
                        term = new_term(cols, rows);
                        app.picker = picker(cols);
                        app.dirty = true;
                    }
                } else if ev["quit"].as_bool() == Some(true) {
                    app.quit = true;
                } else if let Some(k) = ev["key"].as_str().and_then(parse_key) {
                    app.key(k);
                } else if let Some(m) = mouse(&ev) {
                    app.mouse(m);
                }
                app.sync_active();
                app.ensure_table();
            }
        }
        while let Ok(done) = app.resized.try_recv() {
            if let Ok(r) = done {
                app.map.update_resized_protocol(r); // the half-block map shown under popups
                redraw = true;
            }
        }
        if let Some(rx) = &app.asking {
            if let Ok(r) = rx.try_recv() {
                app.asking = None;
                app.finish_ask(r);
                redraw = true;
            }
        }
        if !redraw && drawn.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(25));
            continue;
        }
        (redraw, drawn) = (false, std::time::Instant::now());
        let Ok(_) = term.draw(|f| app.draw(f));
        let frame = frame(term.backend().buffer(), app);
        let key = frame.to_string();
        if key != last {
            frame_no += 1;
            last = key;
            let mut out = frame;
            out["gen"] = json!(frame_no);
            let tmp = dir.join("frame.tmp.json");
            std::fs::write(&tmp, out.to_string())?;
            std::fs::rename(&tmp, dir.join("frame.json"))?;
            std::fs::write(dir.join("gen"), frame_no.to_string())?;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(())
}

/// The screen as Raster cells and as text, and the map's place and picture.
fn frame(buf: &Buffer, app: &App) -> Value {
    let a = buf.area;
    let mut words: Vec<u32> = Vec::with_capacity(a.width as usize * a.height as usize * 3);
    let mut text = Vec::with_capacity(a.height as usize);
    for y in 0..a.height {
        let mut line = String::new();
        for x in 0..a.width {
            let c = &buf[(x, y)];
            let ch = c.symbol().chars().next().unwrap_or(' ');
            // the Raster takes one printable width-1 BMP character per cell
            let ch = if (ch as u32) < 0x20 || (ch as u32) > 0xFFFF || is_wide(ch) { '?' } else { ch };
            let (mut fg, mut bg) = (rgb(c.fg, true), rgb(c.bg, false));
            if c.modifier.contains(Modifier::REVERSED) {
                (fg, bg) = (if bg == DEFAULT { 0x1e1e1e } else { bg }, if fg == DEFAULT { 0xd0d0d0 } else { fg });
            }
            if c.modifier.contains(Modifier::DIM) && fg != DEFAULT {
                fg = (fg >> 1) & 0x7f7f7f;
            } else if c.modifier.contains(Modifier::DIM) {
                fg = 0x808080;
            }
            words.extend([ch as u32, fg, bg]);
            line.push(ch);
        }
        text.push(line.trim_end().to_string());
    }
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let m = app.map_area;
    // While a popup is open the map is drawn in the cells (half blocks), so the popup shows over
    // it: a picture laid over the cells would hide the popup.
    let map = match (&app.serve.as_ref().and_then(|s| s.map_file.clone()), m.width > 0 && !app.overlay_open()) {
        (Some(f), true) => json!({"x": m.x, "y": m.y, "w": m.width, "h": m.height, "file": f.to_string_lossy()}),
        _ => Value::Null,
    };
    json!({"cols": a.width, "rows": a.height, "cells": STANDARD.encode(bytes), "text": text, "map": map})
}

const DEFAULT: u32 = 0x0100_0000;

fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6)
}

/// A ratatui colour as 0x00RRGGBB, or the terminal's default.
fn rgb(c: Color, _fg: bool) -> u32 {
    let named = |i: u8| -> u32 {
        [
            0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666, 0xf14c4c, 0x23d18b,
            0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
        ][i as usize]
    };
    match c {
        Color::Reset => DEFAULT,
        Color::Black => named(0),
        Color::Red => named(1),
        Color::Green => named(2),
        Color::Yellow => named(3),
        Color::Blue => named(4),
        Color::Magenta => named(5),
        Color::Cyan => named(6),
        Color::Gray => named(7),
        Color::DarkGray => named(8),
        Color::LightRed => named(9),
        Color::LightGreen => named(10),
        Color::LightYellow => named(11),
        Color::LightBlue => named(12),
        Color::LightMagenta => named(13),
        Color::LightCyan => named(14),
        Color::White => named(15),
        Color::Rgb(r, g, b) => (r as u32) << 16 | (g as u32) << 8 | b as u32,
        Color::Indexed(i) if i < 16 => named(i),
        Color::Indexed(i) if i >= 232 => {
            let v = 8 + 10 * (i as u32 - 232);
            v << 16 | v << 8 | v
        }
        Color::Indexed(i) => {
            let i = i as u32 - 16;
            let step = |v: u32| if v == 0 { 0 } else { 55 + 40 * v };
            step(i / 36) << 16 | step(i / 6 % 6) << 8 | step(i % 6)
        }
    }
}

/// A key written as the key map writes it (keys::spec): "j", "U", "S-Left", "C-c", "Enter".
pub fn parse_key(s: &str) -> Option<KeyEvent> {
    let (mods, name) = if let Some(rest) = s.strip_prefix("S-") {
        (KeyModifiers::SHIFT, rest)
    } else if let Some(rest) = s.strip_prefix("C-") {
        (KeyModifiers::CONTROL, rest)
    } else {
        (KeyModifiers::NONE, s)
    };
    let code = match name {
        "Space" => KeyCode::Char(' '),
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Tab" => KeyCode::Tab,
        "BackTab" => KeyCode::BackTab,
        "Backspace" => KeyCode::Backspace,
        "Delete" => KeyCode::Delete,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        one if one.chars().count() == 1 => KeyCode::Char(one.chars().next()?),
        _ => return None,
    };
    let mods = if matches!(code, KeyCode::Char(c) if c.is_uppercase()) { mods | KeyModifiers::SHIFT } else { mods };
    Some(KeyEvent::new(code, mods))
}

fn mouse(ev: &Value) -> Option<MouseEvent> {
    let (x, y) = (ev["x"].as_u64()? as u16, ev["y"].as_u64()? as u16);
    let button = if ev["button"] == "right" { MouseButton::Right } else { MouseButton::Left };
    let held = ev["button"].is_string();
    let kind = match ev["mouse"].as_str()? {
        "down" => MouseEventKind::Down(button),
        "up" => MouseEventKind::Up(button),
        "move" if held => MouseEventKind::Drag(button),
        "move" => MouseEventKind::Moved,
        "scroll" if ev["dir"] == "up" => MouseEventKind::ScrollUp,
        "scroll" => MouseEventKind::ScrollDown,
        _ => return None,
    };
    let modifiers = if ev["shift"].as_bool() == Some(true) { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
    Some(MouseEvent { kind, column: x, row: y, modifiers })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        for s in ["j", "U", "S-Left", "C-c", "Enter", "Space", "BackTab", "?", ":", "PageDown"] {
            let k = parse_key(s).unwrap();
            assert_eq!(crate::keys::spec(k), s, "{s}");
        }
        assert_eq!(rgb(Color::Rgb(1, 2, 3), true), 0x010203);
        assert_eq!(rgb(Color::Reset, false), DEFAULT);
        assert_eq!(cell_size(156), (16, 32));
        assert_eq!(cell_size(400), (6, 12));
    }
}
