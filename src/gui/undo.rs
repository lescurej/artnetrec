use crate::show::{Frame, Show};

#[derive(Clone)]
pub enum UndoOp {
    Slot {
        frame: usize,
        slot: usize,
        before: u8,
        after: u8,
    },
    ClearKey {
        frame: usize,
        slot: usize,
        level: u8,
    },
    AddKey {
        frame: usize,
        slot: usize,
        level: u8,
    },
    Time {
        frame: usize,
        before: u32,
        after: u32,
    },
    Insert {
        index: usize,
        frame: Frame,
    },
    Remove {
        index: usize,
        frame: Frame,
    },
    ReplaceFrames {
        before: Vec<Frame>,
        after: Vec<Frame>,
    },
}

pub struct UndoStack {
    undo: Vec<Vec<UndoOp>>,
    redo: Vec<Vec<UndoOp>>,
    limit: usize,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: 48,
        }
    }
}

impl UndoStack {
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn push(&mut self, batch: Vec<UndoOp>) {
        if batch.is_empty() {
            return;
        }
        self.redo.clear();
        self.undo.push(batch);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, show: &mut Show) -> bool {
        let Some(batch) = self.undo.pop() else {
            return false;
        };
        for op in batch.iter().rev() {
            apply(show, op, false);
        }
        self.redo.push(batch);
        true
    }

    pub fn redo(&mut self, show: &mut Show) -> bool {
        let Some(batch) = self.redo.pop() else {
            return false;
        };
        for op in &batch {
            apply(show, op, true);
        }
        self.undo.push(batch);
        true
    }
}

fn apply(show: &mut Show, op: &UndoOp, forward: bool) {
    match op {
        UndoOp::Slot {
            frame,
            slot,
            before,
            after,
        } => {
            let value = if forward { *after } else { *before };
            if let Some(frame) = show.frames.get_mut(*frame) {
                frame.set_key(*slot, value);
            }
        }
        UndoOp::ClearKey { frame, slot, level } => {
            if let Some(frame) = show.frames.get_mut(*frame) {
                if forward {
                    frame.clear_key(*slot);
                } else {
                    frame.set_key(*slot, *level);
                }
            }
        }
        UndoOp::AddKey { frame, slot, level } => {
            if let Some(frame) = show.frames.get_mut(*frame) {
                if forward {
                    frame.set_key(*slot, *level);
                } else {
                    frame.clear_key(*slot);
                }
            }
        }
        UndoOp::Time {
            frame,
            before,
            after,
        } => {
            let value = if forward { *after } else { *before };
            if let Some(f) = show.frames.get_mut(*frame) {
                f.at_ms = value;
            }
        }
        UndoOp::Insert { index, frame } => {
            if forward {
                let idx = (*index).min(show.frames.len());
                show.frames.insert(idx, frame.clone());
            } else if *index < show.frames.len() {
                show.frames.remove(*index);
            }
        }
        UndoOp::Remove { index, frame } => {
            if forward {
                if *index < show.frames.len() {
                    show.frames.remove(*index);
                }
            } else {
                let idx = (*index).min(show.frames.len());
                show.frames.insert(idx, frame.clone());
            }
        }
        UndoOp::ReplaceFrames { before, after } => {
            show.frames = if forward {
                after.clone()
            } else {
                before.clone()
            };
        }
    }
}
