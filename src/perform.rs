//! Playback modes: individual (`P`) and global "perform" (`Shift+P`).
//!
//! Pure state machine + selection dispatch, no SDL: everything here is
//! unit-testable. The `App` wires these into key handling, the top bar,
//! and the transport overlay.

use crate::canvas::{SelKind, VectorMeta};

/// What `P` should act on for the current selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayTarget {
    Vector(u64),
    Clip(u64),
    /// Nothing playable selected: `P` keeps its legacy pencil-tool meaning.
    None,
}

/// Map a selection to its playback target.
pub fn playback_target(sel: Option<(SelKind, u64)>) -> PlayTarget {
    match sel {
        Some((SelKind::Vector, id)) => PlayTarget::Vector(id),
        Some((SelKind::Clip, id)) => PlayTarget::Clip(id),
        _ => PlayTarget::None,
    }
}

/// Toggle the `playing` flag of one vector, leaving all others untouched.
/// Returns true when a vector with `id` was found.
pub fn toggle_vector(vectors: &mut [VectorMeta], id: u64) -> bool {
    match vectors.iter_mut().find(|v| v.id == id) {
        Some(v) => {
            v.playing = !v.playing;
            true
        }
        None => false,
    }
}

/// Global "perform" mode: every animated vector plays at once, every audio
/// clip plays at once. Entering snapshots each vector's `playing` flag so
/// exiting restores exactly what was there before.
pub struct PerformState {
    active: bool,
    saved: Vec<(u64, bool)>, // (vector id, prior playing flag)
}

impl PerformState {
    pub fn new() -> Self {
        Self { active: false, saved: Vec::new() }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Enter perform mode. Snapshots current flags, then sets `playing`
    /// on every vector whose animation kind is animated.
    /// Returns the ids that were started (empty = nothing animated).
    /// Entering while already active is a no-op.
    pub fn enter(&mut self, vectors: &mut [VectorMeta]) -> Vec<u64> {
        if self.active {
            return Vec::new();
        }
        self.saved = vectors.iter().map(|v| (v.id, v.playing)).collect();
        let mut started = Vec::new();
        for v in vectors.iter_mut() {
            if v.anim.animated() {
                v.playing = true;
                started.push(v.id);
            }
        }
        self.active = true;
        started
    }

    /// Exit perform mode, restoring the snapshotted flags. Vectors added
    /// while active keep whatever flag they have. Exiting while inactive
    /// is a no-op.
    pub fn exit(&mut self, vectors: &mut [VectorMeta]) {
        if !self.active {
            return;
        }
        for v in vectors.iter_mut() {
            if let Some((_, was)) = self.saved.iter().find(|(id, _)| *id == v.id) {
                v.playing = *was;
            }
        }
        self.saved.clear();
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::AnimKind;

    fn vec(id: u64, anim: AnimKind, playing: bool) -> VectorMeta {
        VectorMeta {
            id,
            polys: Vec::new(),
            w: 10.0,
            h: 10.0,
            x: 0.0,
            y: 0.0,
            sw: 10.0,
            sh: 10.0,
            color: 0,
            filled: false,
            anim,
            speed: 1.0,
            playing,
            t: 0.0,
        }
    }

    #[test]
    fn enter_starts_only_animated_and_snapshots() {
        let mut vs = vec![
            vec(1, AnimKind::Wave, false),
            vec(2, AnimKind::Still, false), // not animated: untouched
            vec(3, AnimKind::Spin, true),   // already playing: stays
        ];
        let mut p = PerformState::new();
        let started = p.enter(&mut vs);
        assert!(p.is_active());
        assert_eq!(started, vec![1, 3]);
        assert!(vs[0].playing);
        assert!(!vs[1].playing);
        assert!(vs[2].playing);
    }

    #[test]
    fn exit_restores_prior_flags() {
        let mut vs = vec![
            vec(1, AnimKind::Wave, false),
            vec(2, AnimKind::Spin, true),
            vec(3, AnimKind::Still, false),
        ];
        let mut p = PerformState::new();
        p.enter(&mut vs);
        assert!(vs.iter().all(|v| v.playing || !v.anim.animated()));
        p.exit(&mut vs);
        assert!(!p.is_active());
        // exactly the pre-enter flags
        assert!(!vs[0].playing);
        assert!(vs[1].playing);
        assert!(!vs[2].playing);
    }

    #[test]
    fn enter_with_nothing_animated_is_clean_noop() {
        let mut vs = vec![vec(1, AnimKind::Still, false), vec(2, AnimKind::Still, true)];
        let mut p = PerformState::new();
        let started = p.enter(&mut vs);
        assert!(started.is_empty());
        p.exit(&mut vs);
        assert!(!vs[0].playing);
        assert!(vs[1].playing); // untouched throughout
    }

    #[test]
    fn double_enter_does_not_clobber_snapshot() {
        let mut vs = vec![vec(1, AnimKind::Wave, false)];
        let mut p = PerformState::new();
        p.enter(&mut vs);
        let again = p.enter(&mut vs);
        assert!(again.is_empty());
        p.exit(&mut vs);
        assert!(!vs[0].playing); // original snapshot, not the mid-perform flag
    }

    #[test]
    fn exit_without_enter_is_noop() {
        let mut vs = vec![vec(1, AnimKind::Wave, true)];
        let mut p = PerformState::new();
        p.exit(&mut vs); // must not panic, must not touch flags
        assert!(!p.is_active());
        assert!(vs[0].playing);
    }

    #[test]
    fn playback_target_dispatch() {
        assert_eq!(playback_target(Some((SelKind::Vector, 7))), PlayTarget::Vector(7));
        assert_eq!(playback_target(Some((SelKind::Clip, 9))), PlayTarget::Clip(9));
        assert_eq!(playback_target(Some((SelKind::Stroke, 3))), PlayTarget::None);
        assert_eq!(playback_target(Some((SelKind::Image, 3))), PlayTarget::None);
        assert_eq!(playback_target(Some((SelKind::Text, 3))), PlayTarget::None);
        assert_eq!(playback_target(None), PlayTarget::None);
    }

    #[test]
    fn toggle_vector_hits_only_the_target() {
        let mut vs = vec![
            vec(1, AnimKind::Wave, false),
            vec(2, AnimKind::Wave, true),
        ];
        assert!(toggle_vector(&mut vs, 1));
        assert!(vs[0].playing);
        assert!(vs[1].playing); // untouched
        assert!(toggle_vector(&mut vs, 1));
        assert!(!vs[0].playing);
        assert!(!toggle_vector(&mut vs, 999)); // unknown id: false, nothing changes
    }
}
