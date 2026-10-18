//! Stroke drawing-history replay (SDL-free).
//!
//! Selecting a paint stroke replays its drawing: points appear with their
//! recorded timing (true rhythm, pauses included). Strokes recorded before
//! `Stroke.times` existed have empty timing and fall back to a steady pace.
//! The replay is view-only — it never mutates the document.

use crate::canvas::SelKind;

/// Points per second for the steady fallback replay.
pub const STEADY_PTS_PER_SEC: f32 = 60.0;
/// Clamp the steady fallback to a watchable range.
pub const STEADY_MIN_SECS: f32 = 0.4;
pub const STEADY_MAX_SECS: f32 = 6.0;

/// Steady-fallback replay duration for a stroke with no recorded timing.
pub fn steady_replay_duration(n_points: usize) -> f32 {
    (n_points as f32 / STEADY_PTS_PER_SEC).clamp(STEADY_MIN_SECS, STEADY_MAX_SECS)
}

/// How many of `n_points` are visible `elapsed` seconds into a replay.
/// `times` holds seconds-since-stroke-start per point (parallel to points);
/// an empty slice means legacy/unknown timing -> steady fallback.
pub fn replay_visible_count(times: &[f32], n_points: usize, elapsed: f32) -> usize {
    if n_points == 0 {
        return 0;
    }
    if times.is_empty() {
        let d = steady_replay_duration(n_points);
        return ((elapsed / d) * n_points as f32)
            .floor()
            .max(0.0)
            .min(n_points as f32) as usize;
    }
    let n = n_points.min(times.len());
    times[..n].iter().filter(|&&t| t <= elapsed).count()
}

/// Minimal view of app state the replay machine needs (everything Copy).
pub struct ReplayInput {
    pub replay_id: Option<u64>,
    pub selection: Option<(SelKind, u64)>,
    pub replay_done: Option<u64>,
    pub target_gone: bool,
    /// Drawing, dragging, or perform mode — replay never runs while busy.
    pub busy: bool,
    pub replay_finished: bool,
}

/// What the app should do about the replay this frame.
#[derive(Debug, PartialEq, Eq)]
pub enum ReplayCmd {
    None,
    Start(u64),
    Cancel,
    Finish(u64),
}

pub fn replay_command(inp: &ReplayInput) -> ReplayCmd {
    if let Some(rid) = inp.replay_id {
        if inp.target_gone || inp.selection != Some((SelKind::Stroke, rid)) || inp.busy {
            return ReplayCmd::Cancel;
        }
        if inp.replay_finished {
            return ReplayCmd::Finish(rid);
        }
        return ReplayCmd::None;
    }
    if !inp.busy {
        if let Some((SelKind::Stroke, id)) = inp.selection {
            if inp.replay_done != Some(id) {
                return ReplayCmd::Start(id);
            }
        }
    }
    ReplayCmd::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ReplayInput {
        ReplayInput {
            replay_id: None,
            selection: None,
            replay_done: None,
            target_gone: false,
            busy: false,
            replay_finished: false,
        }
    }

    #[test]
    fn steady_duration_pacing_and_clamps() {
        assert_eq!(steady_replay_duration(120), 2.0); // 60 pts/sec
        assert_eq!(steady_replay_duration(6), STEADY_MIN_SECS); // too short -> clamp
        assert_eq!(steady_replay_duration(600), STEADY_MAX_SECS); // too long -> clamp
        assert_eq!(steady_replay_duration(0), STEADY_MIN_SECS);
    }

    #[test]
    fn steady_visible_count_ramps_and_clamps() {
        // 120 points -> 2.0s steady
        assert_eq!(replay_visible_count(&[], 120, 0.0), 0);
        assert_eq!(replay_visible_count(&[], 120, 1.0), 60);
        assert_eq!(replay_visible_count(&[], 120, 1.99), 119);
        assert_eq!(replay_visible_count(&[], 120, 2.0), 120);
        assert_eq!(replay_visible_count(&[], 120, 99.0), 120); // clamped at end
        assert_eq!(replay_visible_count(&[], 0, 5.0), 0);
    }

    #[test]
    fn true_timing_honors_pauses() {
        // point at 0s, point at 0.5s, then a 1.5s pause, point at 2.0s
        let t = [0.0, 0.5, 2.0];
        assert_eq!(replay_visible_count(&t, 3, 0.0), 1);
        assert_eq!(replay_visible_count(&t, 3, 0.5), 2);
        assert_eq!(replay_visible_count(&t, 3, 1.0), 2); // mid-pause: nothing new
        assert_eq!(replay_visible_count(&t, 3, 1.99), 2);
        assert_eq!(replay_visible_count(&t, 3, 2.0), 3);
        assert_eq!(replay_visible_count(&t, 3, 99.0), 3);
    }

    #[test]
    fn true_timing_uses_shorter_of_times_and_points() {
        let t = [0.0, 0.5];
        assert_eq!(replay_visible_count(&t, 5, 99.0), 2);
        assert_eq!(replay_visible_count(&t, 1, 99.0), 1);
    }

    #[test]
    fn arm_on_stroke_select() {
        let mut i = input();
        i.selection = Some((SelKind::Stroke, 7));
        assert_eq!(replay_command(&i), ReplayCmd::Start(7));
    }

    #[test]
    fn no_rearm_after_finish_until_reselect() {
        let mut i = input();
        i.selection = Some((SelKind::Stroke, 7));
        i.replay_done = Some(7);
        assert_eq!(replay_command(&i), ReplayCmd::None);
        // other selections never arm
        i.selection = Some((SelKind::Clip, 7));
        i.replay_done = None;
        assert_eq!(replay_command(&i), ReplayCmd::None);
        i.selection = None;
        assert_eq!(replay_command(&i), ReplayCmd::None);
    }

    #[test]
    fn no_arm_while_busy() {
        let mut i = input();
        i.selection = Some((SelKind::Stroke, 7));
        i.busy = true;
        assert_eq!(replay_command(&i), ReplayCmd::None);
    }

    #[test]
    fn cancel_conditions() {
        // deselect
        let mut i = input();
        i.replay_id = Some(7);
        i.selection = None;
        assert_eq!(replay_command(&i), ReplayCmd::Cancel);
        // select another object
        i.selection = Some((SelKind::Stroke, 8));
        assert_eq!(replay_command(&i), ReplayCmd::Cancel);
        i.selection = Some((SelKind::Clip, 3));
        assert_eq!(replay_command(&i), ReplayCmd::Cancel);
        // target deleted
        i.selection = Some((SelKind::Stroke, 7));
        i.target_gone = true;
        assert_eq!(replay_command(&i), ReplayCmd::Cancel);
        // busy (drawing / dragging / perform)
        i.target_gone = false;
        i.busy = true;
        assert_eq!(replay_command(&i), ReplayCmd::Cancel);
    }

    #[test]
    fn finish_when_visible_complete() {
        let mut i = input();
        i.replay_id = Some(7);
        i.selection = Some((SelKind::Stroke, 7));
        assert_eq!(replay_command(&i), ReplayCmd::None); // still running
        i.replay_finished = true;
        assert_eq!(replay_command(&i), ReplayCmd::Finish(7));
    }
}
