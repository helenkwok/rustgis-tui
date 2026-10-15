//! The key map: one table drives the keys, the `?` help and the footer hints.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ctx {
    Global,
    Map,
    Layers,
    Panel,
    Sketch,
}

impl Ctx {
    pub fn name(self) -> &'static str {
        match self {
            Ctx::Global => "Anywhere",
            Ctx::Map => "Map",
            Ctx::Layers => "Layers",
            Ctx::Panel => "Panel",
            Ctx::Sketch => "Drawing",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Act {
    Quit,
    Help,
    Palette,
    FocusNext,
    FocusPrev,
    Expand,
    TogglePanel,
    Tab(usize),
    Fit,
    Undo,
    Redo,
    SaveAs,
    Ask,
    Redraw,
    Draw,
    Basemap,
    Globe,
    ZoomIn,
    ZoomOut,
    ZoomFull,
    Pan(i8, i8),
    Cross(i8, i8),
    Identify,
    SelectHere,
    ClearCross,
    Highlight,
    LayerMove(i8),
    ToggleVis,
    ZoomLayer,
    Scroll(i32),
    Ends(bool),
    Columns(i8),
    SketchAdd,
    SketchFinish,
    SketchBack,
    SketchCancel,
}

/// (context, keys, action, description, shown in the footer)
pub const KEYMAP: &[(Ctx, &[&str], Act, &str, bool)] = &[
    (Ctx::Sketch, &["click", "Space"], Act::SketchAdd, "add a point (Space: at the crosshair)", true),
    (Ctx::Sketch, &["Enter", "double-click"], Act::SketchFinish, "finish", true),
    (Ctx::Sketch, &["Backspace"], Act::SketchBack, "remove the last point", true),
    (Ctx::Sketch, &["Esc"], Act::SketchCancel, "cancel", true),
    (Ctx::Map, &["arrows", "Left"], Act::Pan(-1, 0), "pan (or drag)", true),
    (Ctx::Map, &["Right"], Act::Pan(1, 0), "", false),
    (Ctx::Map, &["Up"], Act::Pan(0, 1), "", false),
    (Ctx::Map, &["Down"], Act::Pan(0, -1), "", false),
    (Ctx::Map, &["S-Left"], Act::Cross(-1, 0), "move the crosshair (Shift+arrows)", false),
    (Ctx::Map, &["S-Right"], Act::Cross(1, 0), "", false),
    (Ctx::Map, &["S-Up"], Act::Cross(0, -1), "", false),
    (Ctx::Map, &["S-Down"], Act::Cross(0, 1), "", false),
    (Ctx::Map, &["Enter", "click"], Act::Identify, "identify (at the crosshair, or click)", true),
    (Ctx::Map, &["x", "right-click"], Act::SelectHere, "select (right-drag: by box; Shift adds)", true),
    (Ctx::Map, &["Esc"], Act::ClearCross, "hide the crosshair", false),
    (Ctx::Layers, &["Down", "j"], Act::LayerMove(1), "next layer", false),
    (Ctx::Layers, &["Up", "k"], Act::LayerMove(-1), "previous layer", false),
    (Ctx::Layers, &["Space", "click ●"], Act::ToggleVis, "show/hide", true),
    (Ctx::Layers, &["Enter", "double-click"], Act::ZoomLayer, "zoom to the layer", true),
    (Ctx::Panel, &["Down", "j"], Act::Scroll(1), "scroll (wheel)", true),
    (Ctx::Panel, &["Up", "k"], Act::Scroll(-1), "", false),
    (Ctx::Panel, &["PageDown"], Act::Scroll(20), "", false),
    (Ctx::Panel, &["PageUp"], Act::Scroll(-20), "", false),
    (Ctx::Panel, &["g", "Home"], Act::Ends(false), "first row", false),
    (Ctx::Panel, &["G", "End"], Act::Ends(true), "last row", false),
    (Ctx::Panel, &["Right", "l"], Act::Columns(1), "more columns", false),
    (Ctx::Panel, &["Left", "h"], Act::Columns(-1), "", false),
    (Ctx::Global, &["Tab", "click a pane"], Act::FocusNext, "next pane", true),
    (Ctx::Global, &["S-Tab", "BackTab"], Act::FocusPrev, "previous pane", false),
    (Ctx::Global, &["e"], Act::Expand, "full screen for this pane", true),
    (Ctx::Global, &["p"], Act::TogglePanel, "hide/show the panel", false),
    (Ctx::Global, &["1"], Act::Tab(0), "Details", false),
    (Ctx::Global, &["2"], Act::Tab(1), "Messages", false),
    (Ctx::Global, &["3", "t"], Act::Tab(2), "Attributes", false),
    (Ctx::Global, &["+", "=", "wheel"], Act::ZoomIn, "zoom in (wheel: at the pointer)", false),
    (Ctx::Global, &["-"], Act::ZoomOut, "zoom out", false),
    (Ctx::Global, &["0"], Act::ZoomFull, "full extent", false),
    (Ctx::Global, &["f"], Act::Fit, "fit the visible layers", true),
    (Ctx::Global, &["z"], Act::ZoomLayer, "zoom to the chosen layer", false),
    (Ctx::Global, &["Space"], Act::ToggleVis, "show/hide the chosen layer", false),
    (Ctx::Global, &["h"], Act::Highlight, "highlight the chosen layer on the map, on/off", false),
    (Ctx::Global, &["g"], Act::Globe, "globe/flat", false),
    (Ctx::Global, &["b"], Act::Basemap, "next basemap", false),
    (Ctx::Global, &["d"], Act::Draw, "draw or measure", true),
    (Ctx::Global, &[":", "/"], Act::Palette, "every command and tool", true),
    (Ctx::Global, &["u"], Act::Undo, "undo", false),
    (Ctx::Global, &["U"], Act::Redo, "redo", false),
    (Ctx::Global, &["s"], Act::SaveAs, "save as", false),
    (Ctx::Global, &["a"], Act::Ask, "ask the AI assistant", false),
    (Ctx::Global, &["r"], Act::Redraw, "redraw", false),
    (Ctx::Global, &["?"], Act::Help, "keys", true),
    (Ctx::Global, &["q", "C-c"], Act::Quit, "quit", true),
];

/// A key press written the way KEYMAP writes it.
pub fn spec(k: KeyEvent) -> String {
    let base = match k.code {
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(c) if k.modifiers.contains(KeyModifiers::CONTROL) => return format!("C-{c}"),
        KeyCode::Char(c) => return c.to_string(),
        KeyCode::BackTab => return "BackTab".into(),
        KeyCode::F(n) => format!("F{n}"),
        c => format!("{c:?}"),
    };
    if k.modifiers.contains(KeyModifiers::SHIFT) { format!("S-{base}") } else { base }
}

/// The action for a key in the first of `ctxs` that binds it.
pub fn lookup(ctxs: &[Ctx], k: KeyEvent) -> Option<Act> {
    let s = spec(k);
    ctxs.iter().find_map(|c| KEYMAP.iter().find(|e| e.0 == *c && e.1.contains(&s.as_str())).map(|e| e.2))
}

/// The help text, by context.
pub fn help() -> Vec<(Ctx, String, &'static str)> {
    KEYMAP.iter().filter(|e| !e.3.is_empty()).map(|e| (e.0, e.1.join(" "), e.3)).collect()
}

/// Footer hints for the current contexts.
pub fn hints(ctxs: &[Ctx]) -> String {
    ctxs.iter()
        .flat_map(|c| KEYMAP.iter().filter(move |e| e.0 == *c && e.4))
        .map(|e| format!("{} {}", e.1[0], e.3.split(" (").next().unwrap_or(e.3)))
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_resolve_by_context() {
        let k = |c, m| KeyEvent::new(c, m);
        let none = KeyModifiers::NONE;
        assert_eq!(lookup(&[Ctx::Layers, Ctx::Global], k(KeyCode::Enter, none)), Some(Act::ZoomLayer));
        assert_eq!(lookup(&[Ctx::Map, Ctx::Global], k(KeyCode::Enter, none)), Some(Act::Identify));
        assert_eq!(lookup(&[Ctx::Map, Ctx::Global], k(KeyCode::Left, KeyModifiers::SHIFT)), Some(Act::Cross(-1, 0)));
        assert_eq!(lookup(&[Ctx::Panel, Ctx::Global], k(KeyCode::Char('g'), none)), Some(Act::Ends(false)));
        assert_eq!(lookup(&[Ctx::Map, Ctx::Global], k(KeyCode::Char('g'), none)), Some(Act::Globe));
        assert_eq!(lookup(&[Ctx::Global], k(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Act::Quit));
        assert_eq!(lookup(&[Ctx::Global], k(KeyCode::Char('U'), KeyModifiers::SHIFT)), Some(Act::Redo));
        // every bound key resolves to its own entry in its own context
        for e in KEYMAP {
            for key in e.1.iter().filter(|s| !s.contains(' ') && !s.contains("click") && **s != "wheel") {
                let found = KEYMAP.iter().find(|x| x.0 == e.0 && x.1.contains(key)).unwrap();
                assert_eq!(found.2, e.2, "{key} is bound twice in {:?}", e.0);
            }
        }
    }
}
