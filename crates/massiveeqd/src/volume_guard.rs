use massiveeq_core::is_volume_jump;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// A volume raise that was undone.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BlockedJump {
    pub from: f64,
    pub to: f64,
    /// Seconds since the Unix epoch.
    pub at: u64,
}

/// Watches the volumes of outputs whose jumps are blocked. The registry
/// monitor reports every sink's volume; the service chooses which to watch.
#[derive(Default)]
pub struct VolumeGuard {
    state: Mutex<GuardState>,
}

#[derive(Default)]
struct GuardState {
    watched: HashSet<String>,
    levels: HashMap<String, f64>,
    blocked: HashMap<String, BlockedJump>,
}

impl VolumeGuard {
    /// Node names to hold to their level.
    pub fn watch(&self, names: HashSet<String>) {
        self.lock().watched = names;
    }

    /// Record a node's volume level. Returns the level to put back when the
    /// change is a jump on a watched node.
    pub fn observe(&self, node_name: &str, level: f64) -> Option<f64> {
        let mut state = self.lock();
        let previous = state.levels.get(node_name).copied();
        if let Some(previous) = previous
            && state.watched.contains(node_name)
            && is_volume_jump(previous, level)
        {
            let at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |time| time.as_secs());
            state.blocked.insert(
                node_name.to_owned(),
                BlockedJump {
                    from: previous,
                    to: level,
                    at,
                },
            );
            return Some(previous);
        }
        state.levels.insert(node_name.to_owned(), level);
        None
    }

    /// A change MassiveEQ makes itself, which is never a jump.
    pub fn expect(&self, node_name: &str, level: f64) {
        self.lock().levels.insert(node_name.to_owned(), level);
    }

    pub fn last_blocked(&self, node_name: &str) -> Option<BlockedJump> {
        self.lock().blocked.get(node_name).copied()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GuardState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watched_output_keeps_its_level_through_a_jump_but_not_through_key_presses() {
        let guard = VolumeGuard::default();
        guard.watch(HashSet::from(["dac".to_owned()]));
        assert_eq!(guard.observe("dac", 0.30), None);
        assert_eq!(guard.observe("dac", 0.35), None);
        // An app maxes the volume: put it back.
        assert_eq!(guard.observe("dac", 1.0), Some(0.35));
        assert_eq!(guard.last_blocked("dac").map(|jump| jump.to), Some(1.0));
        // The level put back arrives as an ordinary change.
        assert_eq!(guard.observe("dac", 0.35), None);
        assert_eq!(guard.observe("dac", 0.10), None);
    }

    #[test]
    fn massiveeqs_own_raises_pass() {
        let guard = VolumeGuard::default();
        guard.watch(HashSet::from(["dac".to_owned()]));
        guard.observe("dac", 0.3);
        guard.expect("dac", 1.0);
        assert_eq!(guard.observe("dac", 1.0), None);
    }

    #[test]
    fn unwatched_outputs_and_first_reports_are_never_blocked() {
        let guard = VolumeGuard::default();
        assert_eq!(guard.observe("speakers", 0.2), None);
        assert_eq!(guard.observe("speakers", 1.0), None);
        guard.watch(HashSet::from(["headset".to_owned()]));
        assert_eq!(guard.observe("headset", 1.0), None);
        assert_eq!(guard.last_blocked("speakers"), None);
    }
}
