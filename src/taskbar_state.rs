//! Durable native-taskbar recovery state, saved before any shell mutation.
use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TaskbarWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WorkArea {
    pub device: String,
    pub original: [i32; 4],
    pub monitor: [i32; 4],
}

impl WorkArea {
    /// Restore only our own applied geometry, on the same display topology.
    pub fn restore_target(&self, current: &Self) -> Option<[i32; 4]> {
        let [l, t, r, b] = self.original;
        let [ml, mt, mr, mb] = self.monitor;
        (self.device == current.device
            && self.monitor == current.monitor
            && current.original == self.monitor
            && self.original != self.monitor
            && l >= ml
            && t >= mt
            && r <= mr
            && b <= mb
            && l < r
            && t < b)
            .then_some(self.original)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SavedState {
    pub original_abm_state: u32,
    #[serde(default)]
    pub windows: Vec<TaskbarWindow>,
    #[serde(default)]
    pub work_areas: Vec<WorkArea>,
}

pub fn format(state: &SavedState, saved_at: &str) -> String {
    let mut out = format!(
        "# Saved before taskbar changes; retained until restoration succeeds.\noriginal_abm_state = {}\nsaved_at = \"{}\"\n",
        state.original_abm_state, saved_at
    );
    for w in &state.windows {
        out.push_str(&format!("[[windows]]\nhwnd = {}\npid = {}\nvisible = {}\n", w.hwnd, w.pid, w.visible));
    }
    for w in &state.work_areas {
        // Display device names are supplied by Windows; debug escaping is valid TOML.
        out.push_str(&format!(
            "[[work_areas]]\ndevice = {:?}\noriginal = {:?}\nmonitor = {:?}\n",
            w.device, w.original, w.monitor
        ));
    }
    out
}

pub fn parse(text: &str) -> Option<SavedState> {
    toml::from_str(text).ok()
}
pub fn load(path: &Path) -> Option<SavedState> {
    std::fs::read_to_string(path).ok().and_then(|t| parse(&t))
}
pub fn save(path: &Path, state: &SavedState, saved_at: &str) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("toml.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(format(state, saved_at).as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_and_legacy_state() {
        let s = SavedState {
            original_abm_state: 2,
            windows: vec![TaskbarWindow { hwnd: 123, pid: 42, visible: true }],
            work_areas: vec![WorkArea {
                device: "\\\\.\\DISPLAY1".into(),
                original: [60, 0, 1920, 1080],
                monitor: [0, 0, 1920, 1080],
            }],
        };
        assert_eq!(parse(&format(&s, "2026-11-23")), Some(s));
        assert_eq!(parse("original_abm_state = 1").unwrap().windows, []);
    }
    #[test]
    fn garbage_is_none() {
        assert_eq!(parse("original_abm_state = \"x\""), None);
        assert_eq!(parse(""), None);
    }
    #[test]
    fn replacement_is_durable_and_parseable() {
        let p = std::env::temp_dir().join(format!("winshell-taskbar-test-{}.toml", std::process::id()));
        let mut s = SavedState { original_abm_state: 0, windows: vec![], work_areas: vec![] };
        save(&p, &s, "test").unwrap();
        s.original_abm_state = 1;
        save(&p, &s, "test").unwrap();
        assert_eq!(load(&p), Some(s));
        std::fs::remove_file(p).unwrap();
    }
    #[test]
    fn work_area_restore_preserves_other_tools_and_changed_topology() {
        let saved = WorkArea { device: "DISPLAY1".into(), original: [60, 0, 1920, 1080], monitor: [0, 0, 1920, 1080] };
        let mut current = saved.clone();
        current.original = current.monitor;
        assert_eq!(saved.restore_target(&current), Some(saved.original));
        current.original = [0, 30, 1920, 1080];
        assert_eq!(saved.restore_target(&current), None);
        current.original = current.monitor;
        current.monitor = [0, 0, 2560, 1440];
        assert_eq!(saved.restore_target(&current), None);
        let invalid = WorkArea { original: [-100, 0, 1920, 1080], ..saved.clone() };
        assert_eq!(invalid.restore_target(&saved), None);
    }
}
