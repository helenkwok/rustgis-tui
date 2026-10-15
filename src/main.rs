//! rustgis-tui: a terminal UI for RustGIS, driving `rustgis-cli mcp` over stdio.

mod geo;
mod keys;
mod mcp;
mod serve;

use geo::{Canvas, Part, View, fmt_area, fmt_len, merc, path_len, ring_area, unmerc};
use keys::{Act, Ctx};
use mcp::Mcp;
use ratatui::buffer::CellDiffOption;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{DefaultTerminal, Frame};
use ratatui_image::StatefulImage;
use ratatui_image::errors::Errors;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::thread::{ResizeRequest, ResizeResponse, ThreadProtocol};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BASEMAPS: [&str; 5] = ["ocean", "paper", "midnight", "atlas", "blank"];
const TABS: [&str; 3] = ["Details", "Messages", "Attributes"];
/// One accent colour per layer, by layer id: its swatch, its highlight, its table header.
const ACCENTS: [[u8; 3]; 8] = [
    [255, 140, 0],
    [224, 64, 251],
    [0, 229, 255],
    [198, 255, 0],
    [255, 82, 82],
    [68, 138, 255],
    [105, 240, 174],
    [255, 214, 0],
];
const FOCUS: Color = Color::Cyan;

#[derive(Clone)]
struct Item {
    tool: bool, // a geoprocessing tool (run_tool) rather than a command (run_command)
    id: String,
    label: String,
    hint: String, // where it lives / what it takes
    params: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Point,
    Line,
    Polygon,
    Rectangle,
    Circle,
    Distance,
    Area,
}

impl Shape {
    fn from_key(c: char) -> Option<Shape> {
        Some(match c {
            'p' => Shape::Point,
            'l' => Shape::Line,
            'o' => Shape::Polygon,
            'r' => Shape::Rectangle,
            'c' => Shape::Circle,
            'm' => Shape::Distance,
            'M' => Shape::Area,
            _ => return None,
        })
    }
    fn name(self) -> &'static str {
        match self {
            Shape::Point => "point",
            Shape::Line => "line",
            Shape::Polygon => "polygon",
            Shape::Rectangle => "rectangle",
            Shape::Circle => "circle",
            Shape::Distance => "distance",
            Shape::Area => "area",
        }
    }
    /// Shapes that are done after a fixed number of points.
    fn needs(self) -> Option<usize> {
        match self {
            Shape::Point => Some(1),
            Shape::Rectangle | Shape::Circle => Some(2),
            _ => None,
        }
    }
    fn min(self) -> usize {
        match self {
            Shape::Polygon | Shape::Area => 3,
            Shape::Point => 1,
            _ => 2,
        }
    }
    fn closed(self) -> bool {
        matches!(self, Shape::Polygon | Shape::Area)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Layers,
    Map,
    Panel,
}

enum Mode {
    Normal,
    Palette { query: String, sel: usize },
    Params { item: Item, text: String },
    Prompt { ask: bool, text: String },
    Help(u16),
    Info { title: String, lines: Vec<String>, scroll: u16 },
    DrawMenu,
    Sketch { shape: Shape, pts: Vec<(f64, f64)> },
}

struct AttrTable {
    layer: String,
    cols: Vec<String>,
    rows: Vec<Vec<String>>,
    state: TableState,
    col0: usize,
}

struct App {
    cli: String,
    mcp: Mcp,
    picker: Picker,
    yes: bool,
    items: Vec<Item>,
    layers: Vec<Value>,
    layer_sel: ListState,
    active: Option<String>, // the layer RustGIS was last told is active
    header: String,
    projection: String,
    scale: f64,
    basemap: usize,
    log: Vec<String>,
    log_back: usize, // lines scrolled back in Messages
    mode: Mode,
    focus: Focus,
    expanded: bool,
    panel_open: bool,
    tab: usize,
    table: Option<AttrTable>,
    highlight: bool, // the chosen layer is highlighted on the map: off at start, on once one is chosen, h toggles
    // the map image
    map: ThreadProtocol,
    resized: mpsc::Receiver<Result<ResizeResponse, Errors>>,
    base: Option<image::DynamicImage>, // the map as RustGIS drew it
    base_gen: u64,
    shown: Option<(u64, Option<String>, Vec<(f64, f64)>)>, // what the image on screen was made from
    view: Option<View>,
    map_px: (u32, u32),
    dirty: bool,
    geoms: HashMap<String, Rc<Vec<Part>>>, // each layer's shapes, exported once
    // pointer and crosshair
    cross: Option<(u16, u16)>,
    hover: Option<(f64, f64)>,
    drag: Option<(MouseButton, u16, u16)>,
    last_click: Option<(Instant, u16, u16)>,
    // where things were drawn, for mapping clicks
    map_area: Rect,
    layers_area: Rect,
    panel_area: Rect,
    tab_spans: Vec<(u16, u16)>,
    list_area: Rect,
    list_off: usize,
    asking: Option<mpsc::Receiver<Result<String, String>>>,
    quit: bool,
    serve: Option<serve::Serve>, // --serve: no terminal; frames go to a folder (serve.rs)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!(
            "rustgis-tui [--sample | --data FILE ... | --project FILE.rgis] [--safe] [--serve DIR]\n\
             Arguments go to `rustgis-cli mcp`; --sample is the default. File and network access\n\
             (--yes) is on by default, for saving, the full table, layer highlights and the\n\
             assistant; --safe turns it off.\n\
             --serve DIR runs without a terminal, for a Claude Code pane: frames go to DIR, input\n\
             comes from DIR/in (see src/serve.rs).\n\
             RUSTGIS_CLI sets the rustgis-cli path. Press ? inside for the keys."
        );
        return;
    }
    let serve_dir = args.iter().position(|a| a == "--serve").map(|i| {
        let d = args.get(i + 1).cloned().unwrap_or_default();
        args.drain(i..(i + 2).min(args.len()));
        std::path::PathBuf::from(d)
    });
    let safe = args.iter().any(|a| a == "--safe");
    args.retain(|a| a != "--safe");
    if !safe && !args.iter().any(|a| a == "--yes") {
        args.push("--yes".into());
    }
    if !args.iter().any(|a| matches!(a.as_str(), "--sample" | "--data" | "--project")) {
        args.push("--sample".into());
    }
    let cli = std::env::var("RUSTGIS_CLI").unwrap_or_else(|_| "rustgis-cli".into());
    let mcp = match Mcp::start(&cli, &args) {
        Ok(m) => m,
        Err(e) => return eprintln!("rustgis-tui: {e}"),
    };
    let items = load_items(&cli);
    let (serve, picker) = match &serve_dir {
        Some(d) => match serve::start(d) {
            Ok((s, p)) => (Some(s), p),
            Err(e) => return eprintln!("rustgis-tui: {}: {e}", d.display()),
        },
        // When a terminal never answers the graphics query (tmux without passthrough),
        // ratatui-image 11.1's reader thread outlives its timeout and eats the next key, so
        // don't ask inside tmux.
        None if std::env::var_os("TMUX").is_some() => (None, Picker::halfblocks()),
        None => (None, Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())),
    };
    // Encode the map image for the terminal off the UI thread.
    let (tx_work, rx_work) = mpsc::channel::<ResizeRequest>();
    let (tx_done, resized) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(req) = rx_work.recv() {
            if tx_done.send(req.resize_encode()).is_err() {
                break;
            }
        }
    });
    let mut app = App {
        yes: args.iter().any(|a| a == "--yes"),
        cli,
        mcp,
        picker,
        items,
        layers: vec![],
        layer_sel: ListState::default().with_selected(Some(0)),
        active: None,
        header: String::new(),
        projection: String::new(),
        scale: 0.0,
        basemap: 0,
        log: vec!["Press ? for keys. Tab moves between panes.".into()],
        log_back: 0,
        mode: Mode::Normal,
        focus: Focus::Map,
        expanded: false,
        panel_open: true,
        tab: 0,
        table: None,
        highlight: false,
        map: ThreadProtocol::new(tx_work, None),
        resized,
        base: None,
        base_gen: 0,
        shown: None,
        view: None,
        map_px: (0, 0),
        dirty: true,
        geoms: HashMap::new(),
        cross: None,
        hover: None,
        drag: None,
        last_click: None,
        map_area: Rect::default(),
        layers_area: Rect::default(),
        panel_area: Rect::default(),
        tab_spans: vec![],
        list_area: Rect::default(),
        list_off: 0,
        asking: None,
        quit: false,
        serve,
    };
    app.refresh_layers();
    if app.serve.is_some() {
        if let Err(e) = serve::run(&mut app) {
            eprintln!("rustgis-tui: {e}");
        }
        return;
    }
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let res = run(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    if let Err(e) = res {
        eprintln!("rustgis-tui: {e}");
    }
}

/// Every command and tool, read once from the CLI (the same registry the MCP server uses).
fn load_items(cli: &str) -> Vec<Item> {
    let read = |sub: &str| -> Vec<Value> {
        Command::new(cli)
            .arg(sub)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|o| serde_json::from_slice(&o.stdout).ok())
            .unwrap_or_default()
    };
    let mut items: Vec<Item> = read("commands")
        .iter()
        .map(|c| Item {
            tool: false,
            id: s(&c["id"]),
            label: s(&c["label"]),
            hint: s(&c["location"]),
            params: s(&c["params"]),
        })
        .collect();
    items.extend(read("tools").iter().map(|t| {
        let params: Vec<String> = t["params"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let opt = if p["required"].as_bool() == Some(true) { "" } else { "?" };
                format!("\"{}\"{opt}: {}", s(&p["name"]), s(&p["kind"]))
            })
            .collect();
        Item {
            tool: true,
            id: s(&t["id"]),
            label: s(&t["name"]),
            hint: "Geoprocessing tool".into(),
            params: format!("{{{}}}", params.join(", ")),
        }
    }));
    items
}

fn s(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        x => x.to_string(),
    }
}

fn accent(l: &Value) -> [u8; 3] {
    ACCENTS[l["id"].as_u64().unwrap_or(0) as usize % ACCENTS.len()]
}

fn rgb(c: [u8; 3]) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    while !app.quit {
        terminal.draw(|f| app.draw(f))?;
        if let Some(rx) = &app.asking {
            if let Ok(r) = rx.try_recv() {
                app.asking = None;
                app.finish_ask(r);
            }
        }
        while let Ok(done) = app.resized.try_recv() {
            match done {
                Ok(r) => {
                    app.map.update_resized_protocol(r);
                }
                Err(e) => app.note(format!("map image: {e}")),
            }
        }
        if event::poll(Duration::from_millis(40))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.key(k),
                Event::Mouse(m) => app.mouse(m),
                Event::Resize(..) => app.dirty = true,
                _ => {}
            }
            app.sync_active();
            app.ensure_table();
        }
    }
    Ok(())
}

impl App {
    fn note(&mut self, msg: impl Into<String>) {
        let msg: String = msg.into();
        self.log.extend(msg.lines().map(|l| l.chars().take(400).collect::<String>()));
        let n = self.log.len();
        if n > 500 {
            self.log.drain(..n - 500);
        }
        self.log_back = 0;
    }

    fn refresh_layers(&mut self) {
        match self.mcp.call("list_layers", json!({})) {
            Ok(r) => {
                let v: Value = serde_json::from_str(&r.text).unwrap_or_default();
                self.layers = v["layers"].as_array().cloned().unwrap_or_default();
                self.projection = s(&v["projection"]);
                self.scale = v["scale"].as_f64().unwrap_or(0.0);
                if let Some(i) = BASEMAPS.iter().position(|b| *b == v["basemap"]) {
                    self.basemap = i;
                }
                let n = self.layers.len();
                if self.layer_sel.selected().is_none_or(|i| i >= n) {
                    self.layer_sel.select(if n == 0 { None } else { Some(0) });
                }
            }
            Err(e) => self.note(format!("list_layers: {e}")),
        }
        self.dirty = true;
    }

    fn chosen(&self) -> Option<&Value> {
        self.layer_sel.selected().and_then(|i| self.layers.get(i))
    }

    fn layer_name(&self) -> Option<String> {
        self.chosen().map(|l| s(&l["name"]))
    }

    /// Make the layer chosen here RustGIS's active layer too, so commands run from the palette
    /// with no "layer" act on it. One place, whichever key or click changed the choice.
    fn sync_active(&mut self) {
        let name = self.layer_name();
        if name != self.active {
            if let Some(n) = &name {
                if let Err(e) = self.mcp.cmd("layer.select", json!({"layer": n})) {
                    self.note(format!("layer.select: {e}"));
                }
            }
            if self.active.is_some() {
                self.highlight = true; // a layer was chosen (not the first sync at start)
            }
            self.active = name;
        }
    }

    /// Run a command; log what it said unless it was a plain {"ok":true}, then redraw.
    fn cmd(&mut self, id: &str, params: Value) {
        match self.mcp.cmd(id, params) {
            Ok(r) if r.is_error => self.note(format!("{id}: {}", r.text)),
            Ok(r) if !r.text.is_empty() && r.text != r#"{"ok":true}"# => self.note(format!("{id}: {}", r.text)),
            Ok(_) => {}
            Err(e) => self.note(format!("{id}: {e}")),
        }
        if !is_view_only(id) {
            self.data_changed();
        }
        self.refresh_layers();
    }

    /// Something may have changed the data: drop what was read from it.
    fn data_changed(&mut self) {
        self.geoms.clear();
        self.table = None;
    }

    fn extent(&mut self) -> Option<[f64; 4]> {
        let r = self.mcp.cmd("map.extent", json!({})).ok()?;
        let v: Value = serde_json::from_str(&r.text).ok()?;
        let e = v["extent"].as_array()?;
        Some([e[0].as_f64()?, e[1].as_f64()?, e[2].as_f64()?, e[3].as_f64()?])
    }

    fn pan(&mut self, dx: f64, dy: f64) {
        if let Some([x0, y0, x1, y1]) = self.extent() {
            let lon = (x0 + x1) / 2.0 + dx * (x1 - x0) * 0.25;
            let lat = ((y0 + y1) / 2.0 + dy * (y1 - y0) * 0.25).clamp(-85.0, 85.0);
            self.cmd("map.goToXY", json!({"lon": lon, "lat": lat}));
        }
    }

    /// A temporary file name for this process.
    fn tmp(&self, what: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("rustgis-tui-{}-{what}", std::process::id()))
    }

    /// A layer's shapes, exported once as GeoJSON (needs file access).
    fn geom(&mut self, name: &str) -> Option<Rc<Vec<Part>>> {
        if let Some(g) = self.geoms.get(name) {
            return Some(g.clone());
        }
        if !self.yes {
            return None;
        }
        let path = self.tmp("layer.geojson");
        let r = self.mcp.cmd("share.exportLayer", json!({"layer": name, "path": path.to_string_lossy()}));
        let text = std::fs::read_to_string(&path).ok();
        let _ = std::fs::remove_file(&path);
        let parts = match (r, text.and_then(|t| serde_json::from_str::<Value>(&t).ok())) {
            (Ok(r), Some(fc)) if !r.is_error => geo::parts(&fc),
            _ => vec![], // rasters and tables have no shapes: remember that too
        };
        let g = Rc::new(parts);
        self.geoms.insert(name.to_string(), g.clone());
        Some(g)
    }

    fn render_map(&mut self, area: Rect) {
        let fs = self.picker.font_size();
        let px = (
            (area.width as u32 * fs.width as u32).clamp(64, 2048),
            (area.height as u32 * fs.height as u32).clamp(64, 2048),
        );
        if self.dirty || px != self.map_px || self.base.is_none() {
            self.dirty = false;
            self.map_px = px;
            match self.mcp.call("view_map", json!({"width": px.0, "height": px.1})) {
                Ok(r) => {
                    self.header = r.text.lines().next().unwrap_or("").to_string();
                    match r.png.and_then(|b| image::load_from_memory(&b).ok()) {
                        Some(img) => {
                            self.base = Some(img);
                            self.base_gen += 1;
                        }
                        None => self.note(format!("view_map: {}", r.text)),
                    }
                }
                Err(e) => self.note(format!("view_map: {e}")),
            }
            self.view = self.extent().map(|e| View::from_extent(e, px.0 as f64, px.1 as f64));
        }
        // What goes on top of RustGIS's image: the chosen layer's highlight and a sketch.
        let hl = self
            .highlight
            .then(|| self.layer_name())
            .flatten()
            .filter(|_| self.projection == "mercator");
        let (sketch, closed) = match &self.mode {
            Mode::Sketch { shape, pts } => (pts.clone(), shape.closed()),
            _ => (vec![], false),
        };
        let want = (self.base_gen, hl, sketch);
        if self.shown.as_ref() == Some(&want) {
            return;
        }
        let (Some(base), Some(v)) = (self.base.clone(), self.view) else { return };
        let img = if want.1.is_none() && want.2.is_empty() {
            base
        } else {
            let mut im = base.to_rgba8();
            let layer = want.1.as_ref().and_then(|n| self.layers.iter().find(|l| s(&l["name"]) == *n)).cloned();
            if let (Some(n), Some(l)) = (&want.1, layer) {
                if let Some(parts) = self.geom(n) {
                    let mut c = Canvas { im: &mut im, v };
                    c.dim(0.55);
                    c.highlight(&parts, accent(&l));
                }
            }
            Canvas { im: &mut im, v }.sketch(&want.2, closed);
            image::DynamicImage::ImageRgba8(im)
        };
        if let Some(sv) = self.serve.as_mut() {
            sv.save_map(&img); // the front end's picture; the half blocks below show under popups
        }
        self.map.replace_protocol(self.picker.new_resize_protocol(img));
        self.shown = Some(want);
    }

    /// The image pixel at the centre of a terminal cell. The image is never scaled up, so one
    /// cell is one font cell of image pixels.
    fn cell_px(&self, col: u16, row: u16) -> (f64, f64) {
        let fs = self.picker.font_size();
        let a = self.map_area;
        ((col - a.x) as f64 * fs.width as f64 + fs.width as f64 / 2.0, (row - a.y) as f64 * fs.height as f64 + fs.height as f64 / 2.0)
    }

    /// lon, lat at a cell of the map, on the flat (Web Mercator) map only.
    fn geo_at(&self, col: u16, row: u16) -> Option<(f64, f64)> {
        let v = self.view?;
        if self.projection != "mercator" || !self.map_area.contains(Position { x: col, y: row }) {
            return None;
        }
        let (px, py) = self.cell_px(col, row);
        Some(v.geo(px, py))
    }

    fn need_geo(&mut self, col: u16, row: u16) -> Option<(f64, f64)> {
        let g = self.geo_at(col, row);
        if g.is_none() && self.projection != "mercator" {
            self.note("Clicks work on the flat map: press g to switch from the globe.");
        }
        g
    }

    fn go(&mut self, x: f64, y: f64, scale: Option<f64>) {
        let (lon, lat) = unmerc(x, y);
        let mut p = json!({"lon": lon, "lat": lat.clamp(-85.0, 85.0)});
        if let Some(sc) = scale {
            p["scale"] = json!(sc);
        }
        self.cmd("map.goToXY", p);
    }

    /// Wheel zoom: the point under the pointer stays put.
    fn zoom_at(&mut self, col: u16, row: u16, zoom_in: bool) {
        let (Some(v), Some(g)) = (self.view, self.geo_at(col, row)) else {
            return self.cmd(if zoom_in { "map.zoomIn" } else { "map.zoomOut" }, json!({}));
        };
        let f = if zoom_in { 1.0 / 1.5 } else { 1.5 };
        let p = merc(g.0, g.1);
        self.go(p.0 + (v.cx - p.0) * f, p.1 + (v.cy - p.1) * f, Some(self.scale * f));
    }

    fn pan_cells(&mut self, dc: i32, dr: i32) {
        let Some(v) = self.view else { return };
        let fs = self.picker.font_size();
        self.go(v.cx - dc as f64 * fs.width as f64 * v.res, v.cy + dr as f64 * fs.height as f64 * v.res, None);
    }

    /// Fit the map to every visible layer.
    fn fit(&mut self) {
        if !self.yes {
            return self.cmd("map.zoomFull", json!({}));
        }
        let names: Vec<String> =
            self.layers.iter().filter(|l| l["visible"].as_bool() == Some(true)).map(|l| s(&l["name"])).collect();
        let mut b: Option<[f64; 4]> = None;
        for n in names {
            if let Some(e) = self.geom(&n).and_then(|g| geo::bbox(&g)) {
                b = Some(b.map_or(e, |b| [b[0].min(e[0]), b[1].min(e[1]), b[2].max(e[2]), b[3].max(e[3])]));
            }
        }
        match b {
            Some(e) => self.cmd("map.zoomToExtent", json!({"extent": e})),
            None => self.note("No visible layer has shapes to fit."),
        }
    }

    fn identify(&mut self, lon: f64, lat: f64) {
        // Every visible layer at the spot ("topmost", the default, reports only the top layer, so a
        // click on a lake answered with the city drawn over it), the chosen layer first.
        let r = match self.mcp.cmd("identify", json!({"lon": lon, "lat": lat, "mode": "visible"})) {
            Ok(r) => r,
            Err(e) => return self.note(format!("identify: {e}")),
        };
        let v: Value = serde_json::from_str(&r.text).unwrap_or_default();
        let mut results = v["results"].as_array().cloned().unwrap_or_default();
        let chosen = self.layer_name();
        results.sort_by_key(|res| Some(s(&res["layer"])) != chosen); // stable: keeps RustGIS's order otherwise
        let mut lines = vec![];
        for res in &results {
            lines.push(format!("{} · row {}", s(&res["layer"]), s(&res["row"])));
            for (k, val) in res["attributes"].as_object().into_iter().flatten() {
                lines.push(format!("  {k}: {}", s(val)));
            }
        }
        if lines.is_empty() {
            return self.note(format!("Nothing at {lon:.4}, {lat:.4}"));
        }
        let title = format!(" {lon:.4}, {lat:.4} · any key closes ");
        self.mode = Mode::Info { title, lines, scroll: 0 };
    }

    fn select_at(&mut self, lon: f64, lat: f64, add: bool) {
        let mode = if add { "add" } else { "new" };
        self.cmd("select.atPoint", json!({"lon": lon, "lat": lat, "mode": mode}));
    }

    /// Two clicks within half a second on the same cell.
    fn double(&mut self, col: u16, row: u16) -> bool {
        let now = Instant::now();
        let d = matches!(self.last_click, Some((t, c, r)) if c == col && r == row && now - t < Duration::from_millis(450));
        self.last_click = if d { None } else { Some((now, col, row)) };
        d
    }

    /// Finish a sketch: draw it as a graphic, or report the measurement.
    fn finish_sketch(&mut self, shape: Shape, pts: Vec<(f64, f64)>) {
        if pts.len() < shape.min() {
            return self.note(format!("A {} needs at least {} points.", shape.name(), shape.min()));
        }
        match shape {
            Shape::Distance => self.note(format!("Distance: {}", fmt_len(path_len(&pts)))),
            Shape::Area => {
                let mut ring = pts.clone();
                ring.push(pts[0]);
                self.note(format!("Area: {} · perimeter {}", fmt_area(ring_area(&pts)), fmt_len(path_len(&ring))));
            }
            _ => {
                let points: Vec<[f64; 2]> = pts.iter().map(|&(x, y)| [x, y]).collect();
                self.cmd("graphics.add", json!({"shape": shape.name(), "points": points}));
            }
        }
        if self.panel_open {
            self.tab = 1; // the result is in Messages
        }
    }

    /// Add a point to a sketch; returns the sketch, or finishes it when the shape is complete.
    fn sketch_add(&mut self, shape: Shape, mut pts: Vec<(f64, f64)>, g: (f64, f64)) -> Mode {
        pts.push(g);
        if shape.needs() == Some(pts.len()) {
            self.finish_sketch(shape, pts);
            return Mode::Normal;
        }
        Mode::Sketch { shape, pts }
    }

    fn cross_geo(&mut self) -> Option<(f64, f64)> {
        let (c, r) = self.cross?;
        self.need_geo(c, r)
    }

    /// A popup, palette or prompt is drawn over the screen.
    fn overlay_open(&self) -> bool {
        !matches!(self.mode, Mode::Normal | Mode::Sketch { .. })
    }

    fn panes(&self) -> Vec<Focus> {
        let mut v = vec![Focus::Layers, Focus::Map];
        if self.panel_open {
            v.push(Focus::Panel);
        }
        v
    }

    fn ctxs(&self) -> Vec<Ctx> {
        let f = match self.focus {
            Focus::Layers => Ctx::Layers,
            Focus::Map => Ctx::Map,
            Focus::Panel => Ctx::Panel,
        };
        if matches!(self.mode, Mode::Sketch { .. }) {
            vec![Ctx::Sketch, Ctx::Map, Ctx::Global]
        } else {
            vec![f, Ctx::Global]
        }
    }

    // ---------------------------------------------------------------- the attribute table

    /// Load the chosen layer's table when the Attributes tab is showing another one.
    fn ensure_table(&mut self) {
        if !(self.panel_open && self.tab == 2) {
            return;
        }
        let Some(name) = self.layer_name() else { return };
        if self.table.as_ref().is_some_and(|t| t.layer == name) {
            return;
        }
        self.table = self.read_table();
    }

    /// With file access the whole layer is exported to a temporary CSV, so every row and field
    /// shows. Without it, read_features is all there is: the server cuts each reply at about
    /// 8,000 characters and has no offset, so read one column at a time (200 rows of one field
    /// fits), halving the row count if a column still gets cut.
    fn read_table(&mut self) -> Option<AttrTable> {
        let l = self.chosen()?.clone();
        let layer = s(&l["name"]);
        let total = s(&l["features"]);
        let fields: Vec<String> = l["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| s(f).rsplit_once(" (").map_or(s(f), |(n, _)| n.to_string()))
            .collect();
        let table = |cols, rows| AttrTable {
            layer: layer.clone(),
            cols,
            rows,
            state: TableState::default().with_selected(Some(0)),
            col0: 0,
        };
        if self.yes {
            match self.export_csv(&layer) {
                Ok(mut rows) if !rows.is_empty() => {
                    let cols = rows.remove(0);
                    return Some(table(cols, rows));
                }
                Ok(_) => self.note(format!("{layer}: the export was empty; reading rows instead")),
                Err(e) => self.note(format!("{layer}: export failed ({e}); reading rows instead")),
            }
        }
        let mut limit = 200;
        let columns = 'fetch: loop {
            let mut columns = vec![];
            for f in &fields {
                let args = json!({"layer": layer, "fields": [f], "limit": limit});
                let v = match self.mcp.call("read_features", args) {
                    Ok(r) => serde_json::from_str::<Value>(&r.text).ok(),
                    Err(e) => {
                        self.note(format!("read_features: {e}"));
                        return None;
                    }
                };
                match v.and_then(|v| v["rows"].as_array().cloned()) {
                    Some(rows) => columns.push(rows.iter().map(|r| s(&r[f])).collect::<Vec<_>>()),
                    None if limit > 10 => {
                        limit /= 2;
                        continue 'fetch;
                    }
                    None => {
                        self.note(format!("{layer}: the field {f} is too long to show"));
                        return None;
                    }
                }
            }
            break columns;
        };
        let n = columns.first().map_or(0, Vec::len);
        let rows = (0..n).map(|r| columns.iter().map(|c| c[r].clone()).collect()).collect();
        if n.to_string() != total {
            self.note(format!("{layer}: showing {n} of {total} rows (started with --safe: restart without it to see them all)"));
        }
        Some(table(fields, rows))
    }

    fn export_csv(&mut self, layer: &str) -> Result<Vec<Vec<String>>, String> {
        let path = self.tmp("table.csv");
        let r = self.mcp.cmd("share.exportLayer", json!({"layer": layer, "path": path.to_string_lossy()}))?;
        if r.is_error || !path.exists() {
            return Err(r.text);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string());
        let _ = std::fs::remove_file(&path);
        Ok(parse_csv(&text?))
    }

    // ---------------------------------------------------------------- the assistant

    fn start_ask(&mut self, question: String) {
        if !self.yes {
            return self.note("The assistant needs file access: restart without --safe.");
        }
        let path = self.tmp("ask.rgis").to_string_lossy().to_string();
        if let Err(e) = self.mcp.cmd("project.saveAs", json!({"path": path})) {
            return self.note(format!("ask: cannot save the project: {e}"));
        }
        self.note(format!("asking: {question}"));
        self.tab = 1;
        let (tx, rx) = mpsc::channel();
        let cli = self.cli.clone();
        std::thread::spawn(move || {
            let out = Command::new(cli)
                .args(["ask", "--yes", "--project", &path, "--save", &path, &question])
                .stdin(std::process::Stdio::null())
                .output();
            let _ = tx.send(match out {
                Ok(o) if o.status.success() => Ok(format!(
                    "{}{}",
                    String::from_utf8_lossy(&o.stderr),
                    String::from_utf8_lossy(&o.stdout)
                )),
                Ok(o) => Err(String::from_utf8_lossy(&o.stderr).to_string()),
                Err(e) => Err(e.to_string()),
            });
        });
        self.asking = Some(rx);
    }

    fn finish_ask(&mut self, r: Result<String, String>) {
        match r {
            Ok(answer) => {
                self.note(answer);
                let path = self.tmp("ask.rgis");
                self.cmd("project.open", json!({"path": path.to_string_lossy()}));
            }
            Err(e) => self.note(format!("ask failed: {e}")),
        }
    }

    // ---------------------------------------------------------------- keys

    fn key(&mut self, k: KeyEvent) {
        // A Claude Code pane keeps Escape for itself, so there ` stands in for it.
        let k = if self.serve.is_some() && k.code == KeyCode::Char('`') { KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE) } else { k };
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        self.mode = match mode {
            Mode::Normal | Mode::Sketch { .. } => {
                self.mode = mode;
                if let Some(a) = keys::lookup(&self.ctxs(), k) {
                    self.act(a);
                }
                return;
            }
            Mode::Help(scroll) => match k.code {
                KeyCode::Down | KeyCode::Char('j') => Mode::Help(scroll + 1),
                KeyCode::Up | KeyCode::Char('k') => Mode::Help(scroll.saturating_sub(1)),
                _ => Mode::Normal,
            },
            Mode::Info { title, lines, scroll } => match k.code {
                KeyCode::Down => Mode::Info { scroll: (scroll + 1).min(lines.len() as u16), title, lines },
                KeyCode::Up => Mode::Info { scroll: scroll.saturating_sub(1), title, lines },
                _ => Mode::Normal,
            },
            Mode::DrawMenu => match k.code {
                KeyCode::Char(c) => match Shape::from_key(c) {
                    Some(shape) => {
                        self.focus = Focus::Map;
                        if self.cross.is_none() {
                            let a = self.map_area;
                            self.cross = Some((a.x + a.width / 2, a.y + a.height / 2));
                        }
                        Mode::Sketch { shape, pts: vec![] }
                    }
                    None => Mode::Normal,
                },
                _ => Mode::Normal,
            },
            Mode::Palette { mut query, sel } => {
                let n = self.matches(&query).len();
                match k.code {
                    KeyCode::Esc => Mode::Normal,
                    KeyCode::Down => Mode::Palette { query, sel: (sel + 1).min(n.saturating_sub(1)) },
                    KeyCode::Up => Mode::Palette { query, sel: sel.saturating_sub(1) },
                    KeyCode::Enter => match self.matches(&query).get(sel) {
                        Some(&it) => Mode::Params { item: it.clone(), text: "{}".into() },
                        None => Mode::Palette { query, sel },
                    },
                    KeyCode::Backspace => {
                        query.pop();
                        Mode::Palette { query, sel: 0 }
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                        Mode::Palette { query, sel: 0 }
                    }
                    _ => Mode::Palette { query, sel },
                }
            }
            Mode::Params { item, mut text } => match k.code {
                KeyCode::Esc => Mode::Normal,
                KeyCode::Enter => match serde_json::from_str::<Value>(&text) {
                    Ok(p) if p.is_object() => {
                        let r = if item.tool {
                            self.mcp.call("run_tool", json!({"tool": item.id, "params": p}))
                        } else {
                            self.mcp.cmd(&item.id, p)
                        };
                        match r {
                            Ok(r) => self.note(format!("{}: {}", item.id, r.text)),
                            Err(e) => self.note(format!("{}: {e}", item.id)),
                        }
                        if item.tool || !is_view_only(&item.id) {
                            self.data_changed();
                        }
                        self.tab = 1;
                        self.refresh_layers();
                        Mode::Normal
                    }
                    _ => {
                        self.note("Parameters must be a JSON object, e.g. {\"layer\": \"Cities\"}");
                        Mode::Params { item, text }
                    }
                },
                KeyCode::Backspace => {
                    text.pop();
                    Mode::Params { item, text }
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    Mode::Params { item, text }
                }
                _ => Mode::Params { item, text },
            },
            Mode::Prompt { ask, mut text } => match k.code {
                KeyCode::Esc => Mode::Normal,
                KeyCode::Enter if !text.trim().is_empty() => {
                    if ask {
                        self.start_ask(text);
                    } else {
                        self.cmd("project.saveAs", json!({"path": text}));
                    }
                    Mode::Normal
                }
                KeyCode::Backspace => {
                    text.pop();
                    Mode::Prompt { ask, text }
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    Mode::Prompt { ask, text }
                }
                _ => Mode::Prompt { ask, text },
            },
        };
    }

    fn act(&mut self, a: Act) {
        match a {
            Act::Quit if self.serve.is_some() => self.note("Close the pane to stop RustGIS (/rustgis, or ctrl+x x)."),
            Act::Quit => self.quit = true,
            Act::Help => self.mode = Mode::Help(0),
            Act::Palette => self.mode = Mode::Palette { query: String::new(), sel: 0 },
            Act::FocusNext | Act::FocusPrev => {
                let p = self.panes();
                let i = p.iter().position(|f| *f == self.focus).unwrap_or(0);
                let n = p.len();
                self.focus = p[if a == Act::FocusNext { (i + 1) % n } else { (i + n - 1) % n }];
            }
            Act::Expand => self.expanded = !self.expanded,
            Act::TogglePanel => {
                self.panel_open = !self.panel_open;
                if !self.panel_open && self.focus == Focus::Panel {
                    self.focus = Focus::Map;
                }
            }
            Act::Tab(n) => {
                self.panel_open = true;
                self.tab = n;
                if n == 2 {
                    self.focus = Focus::Panel;
                }
            }
            Act::Fit => self.fit(),
            Act::Undo => self.cmd("edit.undo", json!({})),
            Act::Redo => self.cmd("edit.redo", json!({})),
            Act::SaveAs => self.mode = Mode::Prompt { ask: false, text: String::new() },
            Act::Ask if self.asking.is_none() => self.mode = Mode::Prompt { ask: true, text: String::new() },
            Act::Ask => {}
            Act::Redraw => {
                self.data_changed();
                self.refresh_layers();
            }
            Act::Draw => self.mode = Mode::DrawMenu,
            Act::Basemap => {
                let next = BASEMAPS[(self.basemap + 1) % BASEMAPS.len()];
                self.cmd("map.basemap", json!({"name": next}));
            }
            Act::Globe => self.cmd("map.projection", json!({})),
            Act::ZoomIn => self.cmd("map.zoomIn", json!({})),
            Act::ZoomOut => self.cmd("map.zoomOut", json!({})),
            Act::ZoomFull => self.cmd("map.zoomFull", json!({})),
            Act::Pan(dx, dy) => self.pan(dx as f64, dy as f64),
            Act::Cross(dc, dr) => {
                let a = self.map_area;
                let (c, r) = self.cross.unwrap_or((a.x + a.width / 2, a.y + a.height / 2));
                let c = (c as i32 + dc as i32).clamp(a.x as i32, (a.x + a.width).max(1) as i32 - 1) as u16;
                let r = (r as i32 + dr as i32).clamp(a.y as i32, (a.y + a.height).max(1) as i32 - 1) as u16;
                self.cross = Some((c, r));
                self.hover = self.geo_at(c, r);
            }
            Act::Identify => {
                if let Some(g) = self.cross_geo() {
                    self.identify(g.0, g.1);
                } else {
                    self.note("Move the crosshair with Shift+arrows, or click the map.");
                }
            }
            Act::SelectHere => {
                if let Some(g) = self.cross_geo() {
                    self.select_at(g.0, g.1, false);
                }
            }
            Act::ClearCross => self.cross = None,
            Act::Highlight => {
                self.highlight = !self.highlight;
                if self.highlight && !self.yes {
                    self.note("Highlighting needs file access: restart without --safe.");
                }
            }
            Act::LayerMove(d) => {
                if d > 0 {
                    self.layer_sel.select_next()
                } else {
                    self.layer_sel.select_previous()
                }
            }
            Act::ToggleVis => {
                if let Some(name) = self.layer_name() {
                    self.cmd("layer.visibility", json!({"layer": name})); // a pure toggle
                }
            }
            Act::ZoomLayer => {
                if let Some(name) = self.layer_name() {
                    self.cmd("layer.zoomTo", json!({"layer": name}));
                }
            }
            Act::Scroll(n) => match self.tab {
                1 => self.log_back = (self.log_back as i64 - n as i64).max(0) as usize,
                2 => {
                    if let Some(t) = self.table.as_mut() {
                        move_sel(&mut t.state, n as isize, t.rows.len());
                    }
                }
                _ => {}
            },
            Act::Ends(end) => match self.tab {
                1 => self.log_back = if end { 0 } else { usize::MAX / 2 },
                2 => {
                    if let Some(t) = self.table.as_mut() {
                        move_sel(&mut t.state, if end { isize::MAX / 2 } else { isize::MIN / 2 }, t.rows.len());
                    }
                }
                _ => {}
            },
            Act::Columns(d) => {
                if let Some(t) = self.table.as_mut() {
                    t.col0 = (t.col0 as i64 + d as i64).clamp(0, t.cols.len().saturating_sub(1) as i64) as usize;
                }
            }
            Act::SketchAdd | Act::SketchFinish | Act::SketchBack | Act::SketchCancel => {
                let Mode::Sketch { shape, mut pts } = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                self.mode = match a {
                    Act::SketchCancel => Mode::Normal,
                    Act::SketchFinish => {
                        self.finish_sketch(shape, pts);
                        Mode::Normal
                    }
                    Act::SketchBack => {
                        pts.pop();
                        Mode::Sketch { shape, pts }
                    }
                    _ => match self.cross_geo() {
                        Some(g) => self.sketch_add(shape, pts, g),
                        None => Mode::Sketch { shape, pts },
                    },
                };
            }
        }
    }

    // ---------------------------------------------------------------- mouse

    fn mouse(&mut self, m: MouseEvent) {
        let (c, r) = (m.column, m.row);
        let at = |a: Rect| a.contains(Position { x: c, y: r });
        let wheel = match m.kind {
            MouseEventKind::ScrollUp => -1,
            MouseEventKind::ScrollDown => 1,
            _ => 0,
        };
        let down = matches!(m.kind, MouseEventKind::Down(MouseButton::Left));
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        self.mode = match mode {
            Mode::Help(scroll) if wheel != 0 => Mode::Help((scroll as i32 + wheel).max(0) as u16),
            Mode::Help(_) | Mode::Info { .. } if down => Mode::Normal,
            Mode::Info { title, lines, scroll } => {
                let scroll = (scroll as i32 + wheel * 3).clamp(0, lines.len() as i32) as u16;
                Mode::Info { title, lines, scroll }
            }
            Mode::Palette { query, sel } => {
                let n = self.matches(&query).len();
                if wheel != 0 {
                    let sel = (sel as i32 + wheel).clamp(0, n.saturating_sub(1) as i32) as usize;
                    Mode::Palette { query, sel }
                } else if down && at(self.list_area) && r > self.list_area.y {
                    let i = self.list_off + (r - self.list_area.y - 1) as usize;
                    match self.matches(&query).get(i) {
                        Some(&it) => Mode::Params { item: it.clone(), text: "{}".into() },
                        None => Mode::Palette { query, sel },
                    }
                } else {
                    Mode::Palette { query, sel }
                }
            }
            Mode::Normal => {
                self.mouse_main(m, None);
                return;
            }
            Mode::Sketch { shape, pts } => {
                self.mouse_main(m, Some((shape, pts)));
                return;
            }
            other => other,
        };
    }

    /// The mouse on the main screen. `sketch` is an open drawing.
    fn mouse_main(&mut self, m: MouseEvent, sketch: Option<(Shape, Vec<(f64, f64)>)>) {
        let (c, r) = (m.column, m.row);
        let at = |a: Rect| a.contains(Position { x: c, y: r });
        self.mode = match &sketch {
            Some((shape, pts)) => Mode::Sketch { shape: *shape, pts: pts.clone() },
            None => Mode::Normal,
        };
        // Clicking a pane makes it the active one.
        if let MouseEventKind::Down(_) = m.kind {
            if at(self.layers_area) {
                self.focus = Focus::Layers;
            } else if at(self.map_area) {
                self.focus = Focus::Map;
            } else if at(self.panel_area) {
                self.focus = Focus::Panel;
            }
        }
        match m.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => self.hover = self.geo_at(c, r),
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if at(self.map_area) {
                    self.zoom_at(c, r, up);
                } else if at(self.panel_area) {
                    self.act(Act::Scroll(if up { -3 } else { 3 }));
                } else if at(self.layers_area) {
                    self.act(Act::LayerMove(if up { -1 } else { 1 }));
                }
            }
            MouseEventKind::Down(b) if at(self.map_area) => self.drag = Some((b, c, r)),
            MouseEventKind::Down(MouseButton::Left) if at(self.layers_area) && r > self.layers_area.y => {
                let i = self.layer_sel.offset() + (r - self.layers_area.y - 1) as usize / 2;
                if i >= self.layers.len() {
                    return;
                }
                self.layer_sel.select(Some(i));
                if c <= self.layers_area.x + 2 {
                    self.act(Act::ToggleVis);
                } else if self.double(c, r) {
                    self.act(Act::ZoomLayer);
                }
            }
            MouseEventKind::Down(MouseButton::Left) if at(self.panel_area) => {
                if r == self.panel_area.y {
                    if let Some(i) = self.tab_spans.iter().position(|&(x0, x1)| c >= x0 && c < x1) {
                        self.act(Act::Tab(i));
                    }
                } else if self.tab == 2 && r > self.panel_area.y + 1 {
                    let off = self.list_off;
                    if let Some(t) = self.table.as_mut() {
                        let i = off + (r - self.panel_area.y - 2) as usize;
                        if i < t.rows.len() {
                            t.state.select(Some(i));
                        }
                    }
                }
            }
            MouseEventKind::Up(b) => {
                let Some((b0, c0, r0)) = self.drag.take() else { return };
                if b != b0 {
                    return;
                }
                let moved = (c0, r0) != (c, r);
                let add = m.modifiers.contains(KeyModifiers::SHIFT);
                match (b, moved) {
                    (MouseButton::Left, true) => self.pan_cells(c as i32 - c0 as i32, r as i32 - r0 as i32),
                    (MouseButton::Left, false) => {
                        self.cross = Some((c, r));
                        let dbl = self.double(c, r);
                        let Some(g) = self.need_geo(c, r) else { return };
                        match sketch {
                            Some((shape, pts)) if dbl => {
                                // the first click of the double-click already added this point
                                self.finish_sketch(shape, pts);
                                self.mode = Mode::Normal;
                            }
                            Some((shape, pts)) => self.mode = self.sketch_add(shape, pts, g),
                            None => self.identify(g.0, g.1),
                        }
                    }
                    (MouseButton::Right, false) => {
                        if let Some(g) = self.need_geo(c, r) {
                            self.select_at(g.0, g.1, add);
                        }
                    }
                    (MouseButton::Right, true) => {
                        if let (Some(a), Some(z)) = (self.need_geo(c0, r0), self.geo_at(c, r)) {
                            let mode = if add { "add" } else { "new" };
                            self.cmd(
                                "select.byRect",
                                json!({"minLon": a.0.min(z.0), "minLat": a.1.min(z.1),
                                       "maxLon": a.0.max(z.0), "maxLat": a.1.max(z.1), "mode": mode}),
                            );
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn matches(&self, query: &str) -> Vec<&Item> {
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
        self.items
            .iter()
            .filter(|it| {
                let hay = format!("{} {} {}", it.id, it.label, it.hint).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect()
    }

    // ---------------------------------------------------------------- drawing

    fn pane_block(&self, pane: Focus, title: impl Into<Line<'static>>) -> Block<'static> {
        let on = self.focus == pane;
        Block::bordered()
            .title(title)
            .border_type(if on { BorderType::Thick } else { BorderType::Plain })
            .border_style(if on { Style::new().fg(FOCUS) } else { Style::new().fg(Color::DarkGray) })
    }

    fn draw(&mut self, f: &mut Frame) {
        let [top, body, foot] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(5), Constraint::Length(1)]).areas(f.area());
        let busy = if self.asking.is_some() { "  · assistant is working…" } else { "" };
        f.render_widget(Line::from(vec![" RustGIS ".bold().reversed(), format!(" {}{busy}", self.header).into()]), top);

        // Layers on the left; the map over the panel on the right. `e` gives the active pane
        // the whole body.
        let (mut layers_a, mut map_a, mut panel_a) = (Rect::default(), Rect::default(), Rect::default());
        if self.expanded {
            match self.focus {
                Focus::Layers => layers_a = body,
                Focus::Map => map_a = body,
                Focus::Panel => panel_a = body,
            }
        } else {
            let [side, right] = Layout::horizontal([Constraint::Length(36), Constraint::Min(20)]).areas(body);
            layers_a = side;
            if self.panel_open {
                [map_a, panel_a] = Layout::vertical([Constraint::Min(6), Constraint::Length(12)]).areas(right);
            } else {
                map_a = right;
            }
        }
        self.layers_area = layers_a;
        self.panel_area = panel_a;
        self.map_area = Rect::default();
        if !layers_a.is_empty() {
            self.draw_layers(f, layers_a);
        }
        if !map_a.is_empty() {
            self.draw_map(f, map_a);
        }
        if !panel_a.is_empty() {
            self.draw_panel(f, panel_a);
        }

        let hints = keys::hints(&self.ctxs());
        f.render_widget(Line::from(format!(" {hints}")).dim(), foot);
        self.draw_overlay(f);
    }

    fn draw_layers(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .layers
            .iter()
            .map(|l| {
                let vis = if l["visible"].as_bool() == Some(true) { "●" } else { "○" };
                let sel = l["selected"].as_u64().unwrap_or(0);
                let sel = if sel > 0 { format!(" [{sel} sel]") } else { String::new() };
                ListItem::new(vec![
                    Line::from(vec![
                        Span::raw(format!("{vis} ")),
                        Span::styled("■ ", Style::new().fg(rgb(accent(l)))),
                        Span::raw(s(&l["name"])),
                    ]),
                    Line::from(format!("    {} · {}{sel}", s(&l["type"]), s(&l["features"]))).dim(),
                ])
            })
            .collect();
        // The chosen row stays marked when the pane isn't active, just dimmer (as lazygit does).
        let bar = if self.focus == Focus::Layers { Color::DarkGray } else { Color::Indexed(236) };
        let block = self.pane_block(Focus::Layers, " Layers ");
        f.render_stateful_widget(
            List::new(items).block(block).highlight_style(Style::new().bg(bar).add_modifier(Modifier::BOLD)),
            area,
            &mut self.layer_sel,
        );
    }

    fn draw_map(&mut self, f: &mut Frame, area: Rect) {
        let title = match &self.mode {
            Mode::Sketch { shape, pts } => {
                let m = match shape {
                    Shape::Distance | Shape::Line if pts.len() > 1 => format!(" · {}", fmt_len(path_len(pts))),
                    Shape::Area | Shape::Polygon if pts.len() > 2 => format!(" · {}", fmt_area(ring_area(pts))),
                    _ => String::new(),
                };
                Line::from(format!(" Drawing {} · {} points{m} ", shape.name(), pts.len())).fg(Color::Yellow)
            }
            _ => {
                let mut spans = vec![Span::raw(format!(" Map · {} ", BASEMAPS[self.basemap]))];
                if let Some(l) = self.chosen() {
                    let hl = if self.highlight { " (highlighted · h off)" } else { " (h highlights)" };
                    spans.push(Span::raw("· "));
                    spans.push(Span::styled("■ ", Style::new().fg(rgb(accent(l)))));
                    spans.push(Span::raw(format!("{}{hl} ", s(&l["name"]))));
                }
                Line::from(spans)
            }
        };
        let mut block = self.pane_block(Focus::Map, title);
        // Readouts on the bottom border, outside the image: the pointer, the scale and a bar.
        if let Some((x, y)) = self.hover {
            block = block.title_bottom(Line::from(format!(" {x:.5}, {y:.5} ")));
        }
        let mut right = format!(" 1:{} ", group(self.scale.round() as u64));
        if let (Some(v), true) = (self.view, self.projection == "mercator") {
            let fs = self.picker.font_size();
            let per_cell = v.ground_res() * fs.width as f64;
            let (len, label) = geo::nice_len(per_cell * 12.0);
            let cells = (len / per_cell).round().max(2.0) as usize;
            right = format!("{right}├{}┤ {label} ", "─".repeat(cells - 2));
        }
        block = block.title_bottom(Line::from(right).right_aligned());
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.map_area = inner;
        if let Some((c, r)) = self.cross {
            if !inner.contains(Position { x: c, y: r }) {
                self.cross = None;
            }
        }
        self.render_map(inner);
        if self.serve.is_some() && !self.overlay_open() {
            // the front end lays the map picture (serve.rs) over these cells
        } else if self.map.protocol_type().is_some() {
            f.render_stateful_widget(StatefulImage::default(), inner, &mut self.map);
        } else {
            // The next image is still being encoded: leave the terminal's current image alone
            // rather than blanking it for a frame.
            for y in inner.top()..inner.bottom() {
                for x in inner.left()..inner.right() {
                    if let Some(cell) = f.buffer_mut().cell_mut((x, y)) {
                        cell.set_diff_option(CellDiffOption::Skip);
                    }
                }
            }
        }
        // The crosshair: a mark in the cell with half blocks; ticks on the map's border
        // otherwise, since a character over an image breaks the image's row.
        if let Some((c, r)) = self.cross {
            let red = Style::new().fg(Color::LightRed).bold();
            let buf = f.buffer_mut();
            if self.picker.protocol_type() == ProtocolType::Halfblocks {
                if let Some(cell) = buf.cell_mut((c, r)) {
                    cell.set_symbol("╋").set_style(red);
                }
            }
            for (pos, sym) in [((c, area.y), "▼"), ((c, area.bottom() - 1), "▲"), ((area.x, r), "▶"), ((area.right() - 1, r), "◀")] {
                // only over the border line, never over the title or the readouts on it
                if let Some(cell) = buf.cell_mut(pos).filter(|c| matches!(c.symbol(), "─" | "━" | "│" | "┃")) {
                    cell.set_symbol(sym).set_style(red);
                }
            }
        }
    }

    fn draw_panel(&mut self, f: &mut Frame, area: Rect) {
        // Tabs in the top border; remember where each sits, for clicks.
        let mut spans = vec![Span::raw(" ")];
        let mut x = area.x + 2;
        self.tab_spans.clear();
        for (i, t) in TABS.iter().enumerate() {
            let label = format!(" {} {t} ", i + 1);
            let w = label.chars().count() as u16;
            self.tab_spans.push((x, x + w));
            x += w + 1;
            spans.push(if i == self.tab { Span::styled(label, Style::new().reversed().bold()) } else { Span::raw(label) });
            spans.push(Span::raw(" "));
        }
        let block = self.pane_block(Focus::Panel, Line::from(spans));
        let inner = block.inner(area);
        match self.tab {
            0 => {
                let body = match self.chosen() {
                    Some(l) => {
                        let vis = if l["visible"].as_bool() == Some(true) { "shown" } else { "hidden" };
                        let fields: Vec<String> = l["fields"].as_array().into_iter().flatten().map(s).collect();
                        vec![
                            Line::from(vec![
                                Span::styled("■ ", Style::new().fg(rgb(accent(l)))),
                                Span::raw(s(&l["name"])).bold(),
                                Span::raw(format!("  {} · {} features", s(&l["type"]), s(&l["features"]))),
                            ]),
                            Line::from(format!(
                                "{} · {vis} · {} selected",
                                s(&l["renderer"]),
                                l["selected"].as_u64().unwrap_or(0)
                            )),
                            Line::from(format!("fields: {}", fields.join(", "))).dim(),
                        ]
                    }
                    None => vec![Line::from("No layers.")],
                };
                f.render_widget(Paragraph::new(body).wrap(Wrap { trim: true }).block(block), area);
            }
            1 => {
                let h = inner.height as usize;
                let lines: Vec<Line> = self.log.iter().map(|l| Line::from(l.as_str())).collect();
                let n = lines.len();
                self.log_back = self.log_back.min(n.saturating_sub(h));
                let block =
                    if self.log_back > 0 { block.title_bottom(format!(" {} lines back ", self.log_back)) } else { block };
                f.render_widget(
                    Paragraph::new(lines).scroll(((n.saturating_sub(h + self.log_back)) as u16, 0)).block(block),
                    area,
                );
            }
            _ => {
                let colour = |name: &str, layers: &[Value]| {
                    layers.iter().find(|l| s(&l["name"]) == name).map_or(Color::White, |l| rgb(accent(l)))
                };
                let Some(t) = self.table.as_mut() else {
                    let msg = if self.layers.is_empty() { "No layers." } else { "Reading the table…" };
                    return f.render_widget(Paragraph::new(msg).block(block), area);
                };
                let colour = colour(&t.layer, &self.layers);
                let shown: Vec<&String> = t.cols.iter().skip(t.col0).collect();
                let widths: Vec<Constraint> = shown.iter().map(|_| Constraint::Min(12)).collect();
                let header = Row::new(shown.iter().map(|c| Cell::from(c.as_str()))).bold().fg(colour);
                // Build only the rows on screen, so a layer with a million rows stays quick.
                let h = inner.height.saturating_sub(1).max(1) as usize;
                let sel = t.state.selected().unwrap_or(0);
                let off = t.state.offset().min(sel).max((sel + 1).saturating_sub(h));
                *t.state.offset_mut() = off;
                let end = (off + h).min(t.rows.len());
                let body =
                    t.rows[off.min(end)..end].iter().map(|r| Row::new(r.iter().skip(t.col0).map(|c| Cell::from(c.as_str()))));
                let block = block
                    .title_bottom(Line::from(format!(" {} · row {} of {} · ←→ columns ", t.layer, sel + 1, t.rows.len())).fg(colour));
                let mut window = TableState::default().with_selected(Some(sel - off));
                f.render_stateful_widget(
                    Table::new(body, widths).header(header).block(block).row_highlight_style(Style::new().bg(Color::DarkGray)),
                    area,
                    &mut window,
                );
                self.list_off = off;
            }
        }
    }

    fn draw_overlay(&mut self, f: &mut Frame) {
        match &mut self.mode {
            Mode::Normal | Mode::Sketch { .. } => {}
            Mode::Help(scroll) => {
                let mut lines = vec![];
                let mut last = None;
                for (ctx, keys, what) in keys::help() {
                    if last != Some(ctx) {
                        lines.push(Line::from(ctx.name()).bold().fg(FOCUS));
                        last = Some(ctx);
                    }
                    lines.push(Line::from(vec![Span::raw(format!("  {keys:<24}")).bold(), Span::raw(what)]));
                }
                lines.push(Line::from(""));
                lines.push(Line::from("The map is a real image in Ghostty, Kitty, WezTerm and iTerm2; half blocks elsewhere.").dim());
                lines.push(Line::from("Clicks map to coordinates on the flat map only (not the globe).").dim());
                let area = centered(f.area(), 80, 90);
                f.render_widget(Clear, area);
                f.render_widget(
                    Paragraph::new(lines)
                        .scroll((*scroll, 0))
                        .block(Block::bordered().title(" Keys · ↑↓ scroll · any other key closes ")),
                    area,
                );
            }
            Mode::Info { title, lines, scroll } => {
                let h = (lines.len() as u16).min(f.area().height.saturating_sub(4));
                let body: Vec<Line> = lines.iter().map(|l| Line::from(l.as_str())).collect();
                popup(f, 70, h, title, Paragraph::new(body).scroll((*scroll, 0)));
            }
            Mode::DrawMenu => popup(
                f,
                60,
                2,
                " Draw (Esc cancels) ",
                Paragraph::new("p point · l line · o polygon · r rectangle · c circle\nm measure distance · M measure area"),
            ),
            Mode::Palette { query, sel } => {
                let (query, sel) = (query.clone(), *sel);
                let found = self.matches(&query);
                let area = centered(f.area(), 90, 24);
                f.render_widget(Clear, area);
                let rows: Vec<ListItem> = found
                    .iter()
                    .map(|it| {
                        let kind = if it.tool { "tool" } else { "cmd " };
                        ListItem::new(Line::from(vec![
                            Span::raw(format!("{kind} ")).dim(),
                            Span::raw(format!("{:<34}", it.id)).bold(),
                            Span::raw(format!("{}  ", it.label)),
                            Span::raw(it.hint.clone()).dim(),
                        ]))
                    })
                    .collect();
                let title = format!(" {} of {} · type to search · Enter choose · Esc ", found.len(), self.items.len());
                let mut st = ListState::default().with_selected(Some(sel));
                let [q, l] = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
                f.render_widget(Paragraph::new(format!("{query}▏")).block(Block::bordered().title(title)), q);
                f.render_stateful_widget(
                    List::new(rows).block(Block::bordered()).highlight_style(Style::new().bg(Color::DarkGray)),
                    l,
                    &mut st,
                );
                self.list_area = l;
                self.list_off = st.offset();
            }
            Mode::Params { item, text } => {
                let body = vec![
                    Line::from(format!("{}  {}", item.label, item.hint)).bold(),
                    Line::from(format!("takes: {}", item.params)).dim(),
                    Line::from(""),
                    Line::from(format!("{text}▏")),
                ];
                let title = format!(" {} · type JSON · Enter run · Esc ", item.id);
                popup(f, 90, 10, &title, Paragraph::new(body).wrap(Wrap { trim: false }));
            }
            Mode::Prompt { ask, text } => {
                let title = if *ask {
                    " Ask the AI assistant (it may change the project; u undoes) "
                } else {
                    " Save project as (path ending .rgis) "
                };
                popup(f, 80, 3, title, Paragraph::new(format!("{text}▏")));
            }
        }
    }
}

/// Commands that only move the view or the selection, so the layers' data stays as it was.
fn is_view_only(id: &str) -> bool {
    id.starts_with("map.")
        || id.starts_with("select.")
        || matches!(id, "identify" | "layer.visibility" | "layer.zoomTo" | "layer.select")
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn centered(area: Rect, pct_w: u16, h: u16) -> Rect {
    let w = area.width * pct_w / 100;
    let h = if h > 30 { area.height * h / 100 } else { (h + 2).min(area.height) };
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 3, width: w, height: h }
}

fn popup(f: &mut Frame, pct_w: u16, h: u16, title: &str, p: Paragraph) {
    let area = centered(f.area(), pct_w, h);
    f.render_widget(Clear, area);
    f.render_widget(p.block(Block::bordered().title(title.to_string())), area);
}

fn move_sel(state: &mut TableState, by: isize, len: usize) {
    let sel = (state.selected().unwrap_or(0) as isize).saturating_add(by);
    state.select(Some(sel.clamp(0, len.saturating_sub(1) as isize) as usize));
}

/// RFC 4180 CSV: quoted fields may hold commas, doubled quotes and line breaks.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut field) = (vec![], vec![], String::new());
    let (mut quoted, mut chars) = (false, text.chars().peekable());
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, c) => field.push(c),
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut field)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (false, c) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    #[test]
    fn csv() {
        let rows = super::parse_csv("a,b,c\r\n1,\"x, \"\"y\"\"\",\"two\nlines\"\n,,\n");
        assert_eq!(rows, vec![vec!["a", "b", "c"], vec!["1", "x, \"y\"", "two\nlines"], vec!["", "", ""]]);
    }

    #[test]
    fn grouping() {
        assert_eq!(super::group(39266145), "39,266,145");
        assert_eq!(super::group(500), "500");
    }
}
