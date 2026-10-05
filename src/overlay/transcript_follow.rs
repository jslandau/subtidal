//! Native-independent transcript following policy. Layout changes are not reader intent.

#[derive(Clone, Debug)]
pub struct FollowState {
    pub enabled: bool,
    pub pinned: bool,
    pub pending: bool,
    pub generation: u64,
}

impl Default for FollowState {
    fn default() -> Self {
        Self {
            enabled: true,
            pinned: true,
            pending: false,
            generation: 0,
        }
    }
}

pub fn bottom(document: f64, viewport: f64) -> f64 {
    (document - viewport).max(0.0)
}

pub fn at_bottom(value: f64, document: f64, viewport: f64) -> bool {
    let end = bottom(document, viewport);
    end - value.clamp(0.0, end) <= 2.0 + 1e-6
}

impl FollowState {
    pub fn following(&self) -> bool {
        self.enabled && self.pinned
    }
    pub fn status(&self) -> &'static str {
        if !self.enabled {
            "Autoscroll off"
        } else if self.pinned {
            "Following live"
        } else {
            "Paused — reading history"
        }
    }
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
    }
    /// Call only for observed reader movement, never document/layout notifications.
    pub fn observe_user(&mut self, value: f64, document: f64, viewport: f64) {
        self.invalidate();
        self.pinned = at_bottom(value, document, viewport);
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        self.invalidate();
        self.enabled = enabled;
        if enabled {
            self.pinned = true;
        }
    }
    /// Jump once without changing the preference.
    pub fn jump(&mut self) {
        self.invalidate();
        self.pinned = true;
    }
    pub fn clear(&mut self) {
        self.invalidate();
        self.pinned = true;
    }
    /// Coalesce requests. Adapters invalidate superseded callbacks before requesting again.
    pub fn request(&mut self) -> Option<u64> {
        if !self.following() || self.pending {
            return None;
        }
        self.pending = true;
        Some(self.generation)
    }
    pub fn eligible(&self, generation: u64) -> bool {
        self.pending && self.generation == generation && self.following()
    }
    pub fn complete(&mut self, generation: u64) {
        if self.generation == generation {
            self.pending = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn follow_policy_transitions() {
        let mut f = FollowState::default();
        assert_eq!(f.status(), "Following live");
        let g = f.request().unwrap();
        assert!(f.request().is_none());
        f.observe_user(90.0, 200.0, 100.0);
        assert!(!f.eligible(g));
        assert_eq!(f.status(), "Paused — reading history");
        f.observe_user(98.0, 200.0, 100.0);
        assert!(f.following());
        f.set_enabled(false);
        f.jump();
        assert!(!f.following());
        assert!(f.request().is_none());
        f.clear();
        assert!(!f.enabled);
        f.set_enabled(true);
        assert!(f.following());
    }
    #[test]
    fn stale_follow_work_is_invalidated_without_unpinning() {
        let mut state = FollowState::default();
        let old = state.request().unwrap();
        state.invalidate();
        assert!(state.following());
        assert!(!state.eligible(old));
        let latest = state.request().unwrap();
        assert!(state.eligible(latest));
        state.complete(old);
        assert!(state.pending);
        state.complete(latest);
        assert!(!state.pending);
    }
    #[test]
    fn bottom_geometry() {
        assert!(at_bottom(0.0, 50.0, 100.0));
        assert!(at_bottom(110.0, 200.0, 100.0));
        assert!(!at_bottom(97.9, 200.0, 100.0));
        assert!(at_bottom(-10.0, 0.0, 100.0));
    }
}
