//! Running-app model behind the dock (PLAN.md section 6, `DockAppGroup`).
//!
//! Pure data: windows are grouped by app id (AUMID, or lowercased exe path).
//! Group order is stable: existing groups keep their slot, new apps append at
//! the end, and a group disappears when its last window goes (pinning arrives
//! in M4).

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppGroup {
    pub id: String,
    /// Filled in by the asset loader; empty until then.
    pub display_name: String,
    pub exe_path: String,
    pub aumid: Option<String>,
    pub windows: Vec<WindowEntry>,
    /// Index into `windows` of the most recently focused window.
    pub active_window_idx: usize,
}

impl AppGroup {
    fn new(w: &WindowEntry) -> Self {
        Self {
            id: w.app_id.clone(),
            display_name: String::new(),
            exe_path: w.exe_path.clone(),
            aumid: w.aumid.clone(),
            windows: Vec::new(),
            active_window_idx: 0,
        }
    }
}

#[derive(Default, Debug)]
pub struct DockModel {
    groups: Vec<AppGroup>,
    focused: Option<usize>,
}

impl DockModel {
    pub fn groups(&self) -> &[AppGroup] {
        &self.groups
    }

    pub fn group_mut(&mut self, id: &str) -> Option<&mut AppGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// Index of the group that owns the focused window, if it is in the dock.
    pub fn focused(&self) -> Option<usize> {
        self.focused
    }

    pub fn contains(&self, hwnd: isize) -> bool {
        self.locate(hwnd).is_some()
    }

    fn locate(&self, hwnd: isize) -> Option<(usize, usize)> {
        self.groups
            .iter()
            .enumerate()
            .find_map(|(gi, g)| g.windows.iter().position(|w| w.hwnd == hwnd).map(|wi| (gi, wi)))
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
                let mut g = AppGroup::new(&w);
                g.windows.push(w);
                self.groups.push(g);
            }
        }
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
        if g.windows.is_empty() {
            self.groups.remove(gi);
            self.focused = match self.focused {
                Some(f) if f == gi => None,
                Some(f) if f > gi => Some(f - 1),
                f => f,
            };
        }
        true
    }

    /// Full rescan result: keeps the slot order of apps that are still running,
    /// appends new ones, drops vanished ones. Returns true if anything changed.
    pub fn replace_all(&mut self, windows: Vec<WindowEntry>) -> bool {
        let before = self.groups.clone();
        let names: Vec<(String, String)> = self.groups.iter().map(|g| (g.id.clone(), g.display_name.clone())).collect();
        let mut fresh = DockModel::default();
        // Seed existing group order first so slots do not jump around.
        for (id, _) in &names {
            for w in windows.iter().filter(|w| &w.app_id == id) {
                fresh.upsert(w.clone());
            }
        }
        for w in windows {
            if !fresh.contains(w.hwnd) {
                fresh.upsert(w);
            }
        }
        for g in &mut fresh.groups {
            if let Some((_, n)) = names.iter().find(|(id, _)| id == &g.id) {
                g.display_name = n.clone();
            }
        }
        self.groups = fresh.groups;
        self.focused = None;
        self.groups != before
    }

    /// Record the focused top-level window (root owner of the foreground).
    /// Returns true if the focused group changed.
    pub fn set_focused(&mut self, hwnd: isize) -> bool {
        let new = self.locate(hwnd).map(|(gi, wi)| {
            self.groups[gi].active_window_idx = wi;
            gi
        });
        let changed = new != self.focused;
        self.focused = new;
        changed
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
    fn removing_last_window_drops_group() {
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
        assert!(!m.set_focused(2)); // same group
        assert!(m.set_focused(99)); // not in dock
        assert_eq!(m.focused(), None);
    }

    #[test]
    fn focus_index_shifts_when_earlier_group_removed() {
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
        let changed = m.replace_all(vec![win(5, "c"), win(2, "b"), win(4, "b")]);
        assert!(changed);
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
}
