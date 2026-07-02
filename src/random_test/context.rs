//! Generated-value contexts shared by the renderer and relation enforcement.

use std::collections::{HashMap, HashSet};

pub(super) type Ctx = HashMap<String, i64>;
pub(super) type StrCtx = HashMap<String, String>;
pub(super) type ArrayCtx = HashMap<String, Vec<i64>>;

#[derive(Default)]
pub(super) struct RenderContext {
    pub(super) scalars: Ctx,
    pub(super) strings: StrCtx,
    pub(super) arrays: ArrayCtx,
}

pub(super) struct ContextCheckpoint {
    pub(super) scalar_names: HashSet<String>,
    pub(super) array_names: HashSet<String>,
    string_names: HashSet<String>,
    array_lens: HashMap<String, usize>,
}

impl RenderContext {
    pub(super) fn checkpoint(&self) -> ContextCheckpoint {
        ContextCheckpoint {
            scalar_names: self.scalars.keys().cloned().collect(),
            string_names: self.strings.keys().cloned().collect(),
            array_names: self.arrays.keys().cloned().collect(),
            array_lens: self
                .arrays
                .iter()
                .map(|(name, values)| (name.clone(), values.len()))
                .collect(),
        }
    }

    /// Drop values generated inside one repeated block without cloning retained
    /// parent arrays for every iteration. Input formats do not redefine an
    /// outer variable name inside a repeated child block; parent arrays can
    /// therefore be restored by truncating appended values to their old length.
    pub(super) fn restore(&mut self, checkpoint: &ContextCheckpoint) {
        self.scalars
            .retain(|name, _| checkpoint.scalar_names.contains(name));
        self.strings
            .retain(|name, _| checkpoint.string_names.contains(name));
        self.arrays.retain(|name, values| {
            let Some(&len) = checkpoint.array_lens.get(name) else {
                return false;
            };
            values.truncate(len);
            true
        });
    }
}
