// This file is part of htj2k, a safe Rust port of OpenJPH
// (https://github.com/aous72/OpenJPH), which is released under the
// 2-Clause BSD license; see LICENSE-OPENJPH for the original copyright
// notice and licence text, which also apply to this file.

//! Storage that a parameter object keeps for reuse after a restart.

use std::fmt;

/// Objects a parameter object has finished with, kept so that the next
/// object it adds can reuse their storage instead of allocating.
///
/// Spares are storage, not content: whatever takes one out must first
/// give it the state a new object has. They are therefore no part of the
/// parameters: every `Spare` compares equal to every other, a clone is
/// empty, and `Debug` shows only how many there are. This keeps the
/// derived `PartialEq`, `Clone` and `Debug` of the objects that hold
/// them about the parameters alone.
pub(crate) struct Spare<T>(Vec<T>);

impl<T> Spare<T> {
    pub(crate) const fn new() -> Self {
        Spare(Vec::new())
    }

    /// Keeps `item` for reuse.
    pub(crate) fn put(&mut self, item: T) {
        self.0.push(item);
    }

    /// Keeps every item of `items`, which is left empty with its
    /// capacity.
    pub(crate) fn put_all(&mut self, items: &mut Vec<T>) {
        self.0.append(items);
    }

    /// A kept item, in whatever state it was kept, if there is one.
    pub(crate) fn take(&mut self) -> Option<T> {
        self.0.pop()
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// Mutable access to the kept items, for tests that fill them with
    /// garbage.
    #[cfg(test)]
    pub(crate) fn items_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T> Default for Spare<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for Spare<T> {
    /// An empty `Spare`: storage is not copied.
    fn clone(&self) -> Self {
        Self::new()
    }

    /// Leaves `self` as it is: its storage stays where it is.
    fn clone_from(&mut self, _source: &Self) {}
}

impl<T> PartialEq for Spare<T> {
    /// Always true: spares hold no parameters.
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T> fmt::Debug for Spare<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spare").field("len", &self.0.len()).finish()
    }
}
