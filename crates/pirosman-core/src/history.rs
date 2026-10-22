//! Session undo/redo: snapshot stacks around transactional edits.
//!
//! Projects are small (a few KB of JSON), so whole snapshots are cheaper and
//! far safer than inverse operations: undo always restores exactly the state
//! that existed before the edit.

use crate::error::CoreResult;
use crate::model::Project;

pub const UNDO_LIMIT: usize = 200;

#[derive(Debug, Clone, Default)]
pub struct EditHistory {
    undo: Vec<(String, Project)>,
    redo: Vec<(String, Project)>,
}

impl EditHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run an edit on `project`; when it succeeds and changed something, the
    /// previous state becomes undoable and the redo stack is cleared.
    pub fn edit<T>(&mut self, project: &mut Project, label: &str, f: impl FnOnce(&mut Project) -> CoreResult<T>) -> CoreResult<T> {
        let before = project.clone();
        let out = f(project)?;
        if *project != before {
            self.push(label, before);
        }
        Ok(out)
    }

    /// Replace the whole project (restore, branch switch, merge) as one step.
    pub fn replace(&mut self, project: &mut Project, label: &str, next: Project) {
        let before = std::mem::replace(project, next);
        self.push(label, before);
    }

    fn push(&mut self, label: &str, before: Project) {
        self.undo.push((label.to_string(), before));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Undo the last step; returns its label, or None when there is nothing to undo.
    pub fn undo(&mut self, project: &mut Project) -> Option<String> {
        let (label, prev) = self.undo.pop()?;
        let cur = std::mem::replace(project, prev);
        self.redo.push((label.clone(), cur));
        Some(label)
    }

    pub fn redo(&mut self, project: &mut Project) -> Option<String> {
        let (label, next) = self.redo.pop()?;
        let cur = std::mem::replace(project, next);
        self.undo.push((label.clone(), cur));
        Some(label)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|(l, _)| l.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|(l, _)| l.as_str())
    }
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fixtures::*;
    use crate::model::ColorParams;
    use crate::timeline::*;

    #[test]
    fn multi_step_undo_redo_restores_exact_states() {
        let (mut p, mid) = project();
        let mut h = EditHistory::new();
        let (v1, a1) = (p.tracks[0].id, p.tracks[2].id);
        let s0 = p.clone();
        let ids = h.edit(&mut p, "Place", |q| place_clip(q, PlaceClip { media_id: mid, track_id: v1, start: 0, source_in: Some(0), source_out: Some(120) })).unwrap();
        let s1 = p.clone();
        h.edit(&mut p, "Trim", |q| trim_clip(q, ids[0], Edge::Out, 100)).unwrap();
        let s2 = p.clone();
        h.edit(&mut p, "Blade", |q| split_clip(q, ids[0], 40).map(|_| ())).unwrap();
        let s3 = p.clone();
        let mut col = ColorParams::default();
        col.saturation = 1.4;
        h.edit(&mut p, "Grade", |q| update_clip(q, ids[0], ClipPatch { color: Some(col), ..Default::default() })).unwrap();
        let s4 = p.clone();
        h.edit(&mut p, "Mute", |q| update_track(q, a1, TrackPatch { muted: Some(true), ..Default::default() })).unwrap();
        let s5 = p.clone();
        let states = [s0, s1, s2, s3, s4, s5.clone()];
        // A failed edit leaves both the project and the history untouched.
        assert!(h.edit(&mut p, "Bad", |q| trim_clip(q, ids[0], Edge::Out, 9999)).is_err());
        assert_eq!(p, s5);
        assert_eq!(h.undo_label(), Some("Mute"));
        for i in (0..5).rev() {
            assert!(h.undo(&mut p).is_some());
            assert_eq!(p, states[i], "after undo to step {i}");
        }
        assert!(h.undo(&mut p).is_none(), "nothing left to undo");
        for (i, st) in states.iter().enumerate().skip(1) {
            assert!(h.redo(&mut p).is_some());
            assert_eq!(&p, st, "after redo to step {i}");
        }
        assert!(h.redo(&mut p).is_none());
        // A new edit after undo drops the redo branch.
        h.undo(&mut p);
        h.undo(&mut p);
        h.edit(&mut p, "Rename", |q| update_clip(q, ids[0], ClipPatch { name: Some("Opening".into()), ..Default::default() })).unwrap();
        assert!(h.redo_label().is_none());
        assert_eq!(h.undo_label(), Some("Rename"));
    }

    #[test]
    fn no_op_edits_are_not_recorded_and_limit_holds() {
        let (mut p, _) = project();
        let mut h = EditHistory::new();
        let a1 = p.tracks[2].id;
        h.edit(&mut p, "Same", |q| update_track(q, a1, TrackPatch { muted: Some(false), ..Default::default() })).unwrap();
        assert!(h.undo_label().is_none());
        for i in 0..(UNDO_LIMIT + 10) {
            h.edit(&mut p, "Toggle", |q| update_track(q, a1, TrackPatch { muted: Some(i % 2 == 0), ..Default::default() })).unwrap();
        }
        let mut n = 0;
        while h.undo(&mut p).is_some() {
            n += 1;
        }
        assert_eq!(n, UNDO_LIMIT);
    }
}
