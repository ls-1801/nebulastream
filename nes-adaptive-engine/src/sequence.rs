//! Hierarchical sequence number tracking system.
//!
//! Provides support for tracking buffer lineage through pipeline processing
//! using hierarchical sequence numbers (e.g., 1 → 1.1 → 1.1.1).
//!
//! # Examples
//!
//! ```
//! use adaptive_engine::sequence::SequenceNumber;
//!
//! // Create a root sequence number
//! let root = SequenceNumber::new(1);
//!
//! // Create child sequences
//! let child1 = root.child(1); // 1.1
//! let child2 = root.child(2); // 1.2
//! let grandchild = child1.child(1); // 1.1.1
//!
//! // Sequence numbers can be compared
//! assert!(child1 < child2);
//! assert!(child1 < grandchild);
//! ```

use std::fmt;

/// Hierarchical sequence number for tracking buffer lineage.
///
/// Sequence numbers are represented as a series of components,
/// e.g., `[1, 1, 2]` represents the sequence number `1.1.2`.
///
/// Sequence numbers support comparison, which is used for cutoff
/// logic during pipeline replacement: all buffers with sequence
/// numbers greater than the cutoff use the new pipeline version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SequenceNumber {
    components: Vec<u64>,
}

impl SequenceNumber {
    /// Create a new root-level sequence number.
    ///
    /// # Arguments
    ///
    /// * `initial` - The initial sequence number value
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let seq = SequenceNumber::new(1);
    /// assert_eq!(seq.to_string(), "1");
    /// ```
    pub fn new(initial: u64) -> Self {
        Self {
            components: vec![initial],
        }
    }

    /// Create a child sequence number by appending a component.
    ///
    /// This is used when a pipeline emits multiple buffers from
    /// a single input buffer, or when creating hierarchical buffers.
    ///
    /// # Arguments
    ///
    /// * `offset` - The child component to append
    ///
    /// # Returns
    ///
    /// A new sequence number with the offset appended.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let parent = SequenceNumber::new(1);
    /// let child = parent.child(1);
    /// assert_eq!(child.to_string(), "1.1");
    ///
    /// let grandchild = child.child(5);
    /// assert_eq!(grandchild.to_string(), "1.1.5");
    /// ```
    pub fn child(&self, offset: u64) -> Self {
        let mut components = self.components.clone();
        components.push(offset);
        Self { components }
    }

    /// Get the depth (number of components) of this sequence number.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let root = SequenceNumber::new(1);
    /// assert_eq!(root.depth(), 1);
    ///
    /// let child = root.child(2);
    /// assert_eq!(child.depth(), 2);
    /// ```
    pub fn depth(&self) -> usize {
        self.components.len()
    }

    /// Get a reference to the component slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let seq = SequenceNumber::new(1).child(2).child(3);
    /// assert_eq!(seq.components(), &[1, 2, 3]);
    /// ```
    pub fn components(&self) -> &[u64] {
        &self.components
    }

    /// Get the root component as a u64.
    ///
    /// This returns the first (root) component of the sequence number,
    /// which is useful for FFI where a single u64 is needed.
    ///
    /// # Examples
    ///
    /// ```
    /// use adaptive_engine::sequence::SequenceNumber;
    ///
    /// let seq = SequenceNumber::new(42);
    /// assert_eq!(seq.as_u64(), 42);
    ///
    /// let child = seq.child(1);
    /// assert_eq!(child.as_u64(), 42); // Returns root component
    /// ```
    pub fn as_u64(&self) -> u64 {
        self.components.first().copied().unwrap_or(0)
    }
}

impl fmt::Display for SequenceNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.components.iter().map(|c| c.to_string()).collect();
        write!(f, "{}", parts.join("."))
    }
}

impl From<u64> for SequenceNumber {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl From<Vec<u64>> for SequenceNumber {
    fn from(components: Vec<u64>) -> Self {
        Self { components }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_sequence_number() {
        let seq = SequenceNumber::new(42);
        assert_eq!(seq.components(), &[42]);
        assert_eq!(seq.to_string(), "42");
    }

    #[test]
    fn test_child_sequence_number() {
        let root = SequenceNumber::new(1);
        let child1 = root.child(1);
        let child2 = root.child(2);
        let grandchild = child1.child(3);

        assert_eq!(child1.to_string(), "1.1");
        assert_eq!(child2.to_string(), "1.2");
        assert_eq!(grandchild.to_string(), "1.1.3");
    }

    #[test]
    fn test_sequence_number_comparison() {
        let seq1_1 = SequenceNumber::new(1).child(1);
        let seq1_2 = SequenceNumber::new(1).child(2);
        let seq1_1_1 = seq1_1.child(1);

        assert!(seq1_1 < seq1_2);
        assert!(seq1_1 < seq1_1_1);
        assert!(seq1_2 > seq1_1);
    }

    #[test]
    fn test_sequence_number_depth() {
        let root = SequenceNumber::new(1);
        assert_eq!(root.depth(), 1);

        let child = root.child(1);
        assert_eq!(child.depth(), 2);

        let grandchild = child.child(2);
        assert_eq!(grandchild.depth(), 3);
    }

    #[test]
    fn test_from_conversions() {
        let seq1: SequenceNumber = 42.into();
        assert_eq!(seq1.to_string(), "42");

        let seq2: SequenceNumber = vec![1, 2, 3].into();
        assert_eq!(seq2.to_string(), "1.2.3");
    }
}
