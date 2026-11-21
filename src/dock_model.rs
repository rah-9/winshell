//! Dock model (PLAN.md section 6, `DockAppGroup`).
//!
//! Pure data. Windows are grouped by app id (AUMID, or lowercased exe path).
//! Order: pinned apps first, in pin order (they stay even with no windows),
//! then running unpinned apps in the order they appeared. A running group
//! keeps its slot; an unpinned group disappears with its last window.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowEntry {
    pub hwnd: isize,
    /// Grouping key: AUMID if known, else lowercased exe path.
    pub app_id: String,
    pub exe_path: String,
    pub aumid: Option<String>,
    pub title: String,
    pub process_id: u32,
    pub is_elevated: bool,
}

/// A pinned app as resolved from `config.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinSpec {
    pub app_id: String,
    pub exe_path: String,
    pub aumid: Option<String>,
    /// The exact string in `config.toml` (used to unpin).
    pub entry: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppGroup {
    pub id: String,
    /// Filled in by the asset loader; empty until then.
    pub display_name: String,
    pub exe_path: String,
    pub aumid: Option<String>,
    pub is_pinned: bool,
    pub windows: Vec<WindowEntry>,
    /// Index into `windows` of the most recently focused window.
    pub active_window_idx: usize,
}

impl AppGroup {
    fn from_window(w: &WindowEntry) -> Self {
        Self {
            id: w.app_id.clone(),
            display_name: String::new(),
            exe_path: w.exe_path.clone(),
            aumid: w.aumid.clone(),
            is_pinned: false,
            windows: Vec::new(),
            active_window_idx: 0,
        }
    }

    fn from_pin(p: &PinSpec) -> Self {
        Self {
            id: p.app_id.clone(),
            display_name: String::new(),
            exe_path: p.exe_path.clone(),
            aumid: p.aumid.clone(),
            is_pinned: true,
            windows: Vec::new(),
            active_window_idx: 0,
        }
    }

    pub fn is_running(&self) -> bool {
        !self.windows.is_empty()
    }

    /// The window to act on for a click: the most recently focused one.
    pub fn active_window(&self) -> Option<&WindowEntry> {
        self.windows.get(self.active_window_idx).or_else(|| self.windows.first())
    }
}

#[derive(Default, Debug)]
pub struct DockModel {
    groups: Vec<AppGroup>,
    pins: Vec<PinSpec>,
    focused_hwnd: isize,
}

impl DockModel {
    pub fn groups(&self) -> &[AppGroup] {
        &self.groups
    }

    pub fn pins(&self) -> &[PinSpec] {
        &self.pins
    }

    pub fn group_mut(&mut self, id: &str) -> Option<&mut AppGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// Number of leading pinned groups (for the separator after them).
    pub fn pinned_count(&self) -> usize {
        self.groups.iter().take_while(|g| g.is_pinned).count()
    }

    pub fn pin_for(&self, id: &str) -> Option<&PinSpec> {
        self.pins.iter().find(|p| p.app_id == id)
    }

    /// Index of the group that owns the focused window, if it is in the dock.
    pub fn focused(&self) -> Option<usize> {
        self.locate(self.focused_hwnd).map(|(gi, _)| gi)
    }

    pub fn contains(&self, hwnd: isize) -> bool {
        self.locate(hwnd).is_some()
    }

    fn locate(&self, hwnd: isize) -> Option<(usize, usize)> {
        if hwnd == 0 {
            return None;
        }
        self.groups
            .iter()
            .enumerate()
            .find_map(|(gi, g)| g.windows.iter().position(|w| w.hwnd == hwnd).map(|wi| (gi, wi)))
    }

    /// Pinned groups first in pin order, then the rest in their current order;
    /// every pin has a group; unpinned groups without windows are dropped.
    fn normalize(&mut self) {
        for g in &mut self.groups {
            g.is_pinned = self.pins.iter().any(|p| p.app_id == g.id);
        }
        for p in &self.pins {
            if !self.groups.iter().any(|g| g.id == p.app_id) {
                self.groups.push(AppGroup::from_pin(p));
            }
        }
        self.groups.retain(|g| g.is_pinned || !g.windows.is_empty());
        let pins = &self.pins;
        let rank = |g: &AppGroup| pins.iter().position(|p| p.app_id == g.id).unwrap_or(usize::MAX);
        self.groups.sort_by_key(rank); // stable: running apps keep their relative order
    }

    /// Replace the pin list (config load or pin/unpin). Returns true if the dock changed.
    pub fn set_pins(&mut self, pins: Vec<PinSpec>) -> bool {
        let before = self.groups.clone();
        self.pins = pins;
        self.normalize();
        self.groups != before
    }

    /// Insert or update a window. Returns true if anything visible changed.
    pub fn upsert(&mut self, w: WindowEntry) -> bool {
        if let Some((gi, wi)) = self.locate(w.hwnd) {
            if self.groups[gi].id == w.app_id {
                let changed = self.groups[gi].windows[wi] != w;
                self.groups[gi].windows[wi] = w;
                return changed;
            }
            // App identity changed (e.g. a UWP frame resolved its hosted app): move it.
            self.remove(w.hwnd);
        }
        match self.groups.iter().position(|g| g.id == w.app_id) {
            Some(gi) => self.groups[gi].windows.push(w),
            None => {
                let mut g = AppGroup::from_window(&w);
                g.windows.push(w);
                self.groups.push(g);
            }
        }
        self.normalize();
        true
    }

    /// Remove a window. Returns true if it was present.
    pub fn remove(&mut self, hwnd: isize) -> bool {
        let Some((gi, wi)) = self.locate(hwnd) else { return false };
        let g = &mut self.groups[gi];
        g.windows.remove(wi);
        if g.active_window_idx >= g.windows.len() {
            g.active_window_idx = g.windows.len().saturating_sub(1);
        }
        self.normalize();
        true
    }

    /// Full rescan result: keeps the slot order of apps that are still running,
    /// appends new ones, drops vanished ones. Returns true if anything changed.
    pub fn replace_all(&mut self, windows: Vec<WindowEntry>) -> bool {
        let before = self.groups.clone();
        let mut fresh: Vec<AppGroup> =
            self.groups.iter().map(|g| AppGroup { windows: Vec::new(), active_window_idx: 0, ..g.clone() }).collect();
        for w in windows {
            match fresh.iter().position(|g| g.id == w.app_id) {
                Some(gi) => fresh[gi].windows.push(w),
                None => {
                    let mut g = AppGroup::from_window(&w);
                    g.windows.push(w);
                    fresh.push(g);
                }
            }
        }
        self.groups = fresh;
        self.normalize();
        self.groups != before
    }

    /// Record the focused top-level window (root owner of the foreground).
    /// Returns true if the focused group changed.
    pub fn set_focused(&mut self, hwnd: isize) -> bool {
        let before = self.focused();
        if let Some((gi, wi)) = self.locate(hwnd) {
            self.groups[gi].active_window_idx = wi;
        }
        self.focused_hwnd = hwnd;
        self.focused() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(hwnd: isize, app: &str) -> WindowEntry {
        WindowEntry {
            hwnd,
            app_id: app.into(),
            exe_path: format!("c:\\{app}.exe"),
            aumid: None,
            title: format!("{app} {hwnd}"),
            process_id: 1,
            is_elevated: false,
        }
    }

    fn pin(app: &str) -> PinSpec {
        PinSpec { app_id: app.into(), exe_path: format!("c:\\{app}.exe"), aumid: None, entry: app.into() }
    }

    fn ids(m: &DockModel) -> Vec<&str> {
        m.groups().iter().map(|g| g.id.as_str()).collect()
    }

    #[test]
    fn groups_windows_by_app() {
        let mut m = DockModel::default();
        m.upsert(win(1, "a"));
        m.upsert(win(2, "b"));
        m.upsert(win(3, "a"));
        assert_eq!(ids(&m), ["a", "b"]);
        assert_eq!(m.groups()[0].windows.len(), 2);
    }

    #[test]
    fn upsert_same_window_reports_change_only_when_different() {
        let mut m = DockModel::default();
        assert!(m.upsert(win(1, "a")));
        assert!(!m.upsert(win(1, "a")));
        let mut w = win(1, "a");
        w.title = "renamed".into();
        assert!(m.upsert(w));
    }

    #[test]
    fn removing_last_window_drops_unpinned_group() {
        let mut m = DockModel::default();
        m.upsert(win(1, "a"));
        m.upsert(win(2, "b"));
        assert!(m.remove(1));
        assert_eq!(ids(&m), ["b"]);
        assert!(!m.remove(1));
    }

    #[test]
    fn identity_change_moves_window() {
        let mut m = DockModel::default();
        m.upsert(win(1, "framehost"));
        m.upsert(win(1, "calc"));
        assert_eq!(ids(&m), ["calc"]);
    }

    #[test]
    fn focus_tracks_group_and_active_window() {
        let mut m = DockModel::default();
        m.upsert(win(1, "a"));
        m.upsert(win(2, "b"));
        m.upsert(win(3, "b"));
        assert!(m.set_focused(3));
        assert_eq!(m.focused(), Some(1));
        assert_eq!(m.groups()[1].active_window_idx, 1);
        assert_eq!(m.groups()[1].active_window().unwrap().hwnd, 3);
        assert!(!m.set_focused(2)); // same group
        assert!(m.set_focused(99)); // not in dock
        assert_eq!(m.focused(), None);
    }

    #[test]
    fn focus_follows_window_when_slots_shift() {
        let mut m = DockModel::default();
        m.upsert(win(1, "a"));
        m.upsert(win(2, "b"));
        m.set_focused(2);
        m.remove(1);
        assert_eq!(m.focused(), Some(0));
        assert_eq!(m.groups()[0].id, "b");
    }

    #[test]
    fn replace_all_keeps_order_and_names() {
        let mut m = DockModel::default();
        m.upsert(win(1, "a"));
        m.upsert(win(2, "b"));
        m.group_mut("b").unwrap().display_name = "Bee".into();
        assert!(m.replace_all(vec![win(5, "c"), win(2, "b"), win(4, "b")]));
        assert_eq!(ids(&m), ["b", "c"]);
        assert_eq!(m.groups()[0].display_name, "Bee");
        assert_eq!(m.groups()[0].windows.len(), 2);
    }

    #[test]
    fn replace_all_unchanged_reports_false() {
        let mut m = DockModel::default();
        m.upsert(win(1, "a"));
        assert!(!m.replace_all(vec![win(1, "a")]));
    }

    #[test]
    fn pins_come_first_in_pin_order_and_persist_without_windows() {
        let mut m = DockModel::default();
        m.upsert(win(1, "running"));
        m.upsert(win(2, "b"));
        m.set_pins(vec![pin("b"), pin("a")]);
        assert_eq!(ids(&m), ["b", "a", "running"]);
        assert_eq!(m.pinned_count(), 2);
        assert!(m.groups()[0].is_running() && !m.groups()[1].is_running());
        m.remove(2);
        assert_eq!(ids(&m), ["b", "a", "running"]);
        assert!(!m.groups()[0].is_running());
    }

    #[test]
    fn unpinning_an_idle_app_removes_it_but_keeps_running_ones() {
        let mut m = DockModel::default();
        m.upsert(win(1, "a"));
        m.set_pins(vec![pin("a"), pin("idle")]);
        assert!(m.set_pins(vec![]));
        assert_eq!(ids(&m), ["a"]);
        assert!(!m.groups()[0].is_pinned);
    }

    #[test]
    fn new_window_of_pinned_app_joins_pinned_slot() {
        let mut m = DockModel::default();
        m.set_pins(vec![pin("a")]);
        m.upsert(win(1, "z"));
        m.upsert(win(2, "a"));
        assert_eq!(ids(&m), ["a", "z"]);
        assert_eq!(m.groups()[0].windows.len(), 1);
    }

    #[test]
    fn replace_all_keeps_pins() {
        let mut m = DockModel::default();
        m.set_pins(vec![pin("a")]);
        m.replace_all(vec![win(1, "b")]);
        assert_eq!(ids(&m), ["a", "b"]);
    }
}
