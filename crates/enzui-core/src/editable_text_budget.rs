//! Measured admission policy for eager text-editor buffers.
//!
//! Admission is bounded by source bytes, estimated resident cost, and longest logical line. Scanning retains no source content and reports when a bounded alternative is required.

use std::io::{self, Read};

/// Calibrated upper bounds for admitting a document to an eager editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditableTextBudget {
    /// Largest source that may be eagerly read at all.
    pub max_source_bytes: u64,
    /// Maximum estimated document-owned resident bytes.
    pub max_resident_bytes: u64,
    /// Resident bytes charged for each source byte (for duplicate owners).
    pub resident_bytes_per_source_byte: u64,
    /// Resident bytes charged for each logical line (metadata + allocation).
    pub resident_bytes_per_logical_line: u64,
    /// Largest logical line the eager shaper may receive.
    pub max_line_bytes: u64,
}

/// Shape counters collected without retaining source contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditableTextShape {
    /// Source length charged by the policy: the greater of metadata and bytes seen.
    pub source_bytes: u64,
    /// Logical lines observed. This is a lower bound when scanning stopped.
    pub logical_lines: u64,
    /// Longest logical line observed, excluding its terminator.
    pub longest_line_bytes: u64,
    /// Source bytes actually inspected before the decision.
    pub inspected_bytes: u64,
}

/// Why an eager editor was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditableTextRefusal {
    /// The charged source length exceeded the source-size ceiling.
    SourceBytes,
    /// A logical line exceeded the shaper's paragraph bound.
    LineBytes,
    /// Calibrated document-owned resident cost exceeded the budget.
    ResidentBytes,
}

/// Controls whether scanning continues to find a stricter overlong-line refusal after resident cost is exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentStop {
    /// Stop at the first certain resident refusal (cheapest classification).
    StopEarly,
    /// Continues scanning so an overlong line can take precedence over resident-cost refusal.
    ContinueForLineBound,
}

/// Result of applying an [`EditableTextBudget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditableTextDecision {
    /// The complete scan stayed within every bound.
    Editable {
        /// Complete document shape.
        shape: EditableTextShape,
        /// Calibrated resident estimate.
        estimated_resident_bytes: u64,
    },
    /// The eager representation was refused and requires a bounded alternative.
    Paged {
        /// Refusal selected under the requested scanning policy.
        reason: EditableTextRefusal,
        /// Complete or early-stop shape counters.
        shape: EditableTextShape,
        /// Calibrated resident estimate at refusal.
        estimated_resident_bytes: u64,
    },
}

impl EditableTextDecision {
    /// Whether the candidate may enter the eager editor.
    #[must_use]
    pub fn is_editable(self) -> bool {
        matches!(self, Self::Editable { .. })
    }

    /// Shape counters attached to either decision.
    #[must_use]
    pub fn shape(self) -> EditableTextShape {
        match self {
            Self::Editable { shape, .. } | Self::Paged { shape, .. } => shape,
        }
    }

    /// Calibrated resident estimate attached to either decision.
    #[must_use]
    pub fn estimated_resident_bytes(self) -> u64 {
        match self {
            Self::Editable {
                estimated_resident_bytes,
                ..
            }
            | Self::Paged {
                estimated_resident_bytes,
                ..
            } => estimated_resident_bytes,
        }
    }
}

impl EditableTextBudget {
    /// Estimate document-owned resident bytes for a source shape.
    #[must_use]
    pub fn estimate_resident_bytes(self, source_bytes: u64, logical_lines: u64) -> u64 {
        source_bytes
            .saturating_mul(self.resident_bytes_per_source_byte)
            .saturating_add(logical_lines.saturating_mul(self.resident_bytes_per_logical_line))
    }

    /// Assesses a source without retaining its contents.
    ///
    /// Metadata length is charged before reading; an oversized value leaves the reader untouched. Scanning stops when refusal is certain, and CRLF or LFCR counts as one terminator.
    ///
    /// # Errors
    ///
    /// Returns an error from the supplied reader.
    pub fn assess_reader(
        self,
        source_bytes: u64,
        reader: impl Read,
    ) -> io::Result<EditableTextDecision> {
        self.assess_reader_stopping(source_bytes, reader, ResidentStop::StopEarly)
    }

    /// Assesses a source with an explicit post-resident-refusal stop policy; either policy uses only the fixed scan buffer.
    ///
    /// # Errors
    ///
    /// Returns an error from the supplied reader.
    pub fn assess_reader_stopping(
        self,
        source_bytes: u64,
        mut reader: impl Read,
        resident_stop: ResidentStop,
    ) -> io::Result<EditableTextDecision> {
        let mut shape = EditableTextShape {
            source_bytes,
            logical_lines: 1,
            longest_line_bytes: 0,
            inspected_bytes: 0,
        };
        if source_bytes > self.max_source_bytes {
            return Ok(self.refused(EditableTextRefusal::SourceBytes, shape));
        }
        let mut resident_refused =
            self.estimate_resident_bytes(source_bytes, 1) > self.max_resident_bytes;
        if resident_refused && resident_stop == ResidentStop::StopEarly {
            return Ok(self.refused(EditableTextRefusal::ResidentBytes, shape));
        }

        let mut current_line = 0u64;
        let mut prior_break = None;
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let used = reader.read(&mut chunk)?;
            if used == 0 {
                break;
            }
            for &byte in &chunk[..used] {
                shape.inspected_bytes = shape.inspected_bytes.saturating_add(1);
                shape.source_bytes = shape.source_bytes.max(shape.inspected_bytes);
                if shape.source_bytes > self.max_source_bytes {
                    return Ok(self.refused(EditableTextRefusal::SourceBytes, shape));
                }
                if matches!(byte, b'\r' | b'\n') {
                    let paired = prior_break.is_some_and(|prior| prior != byte);
                    prior_break = if paired { None } else { Some(byte) };
                    if paired {
                        continue;
                    }
                    shape.longest_line_bytes = shape.longest_line_bytes.max(current_line);
                    current_line = 0;
                    shape.logical_lines = shape.logical_lines.saturating_add(1);
                    if self.estimate_resident_bytes(shape.source_bytes, shape.logical_lines)
                        > self.max_resident_bytes
                    {
                        if resident_stop == ResidentStop::StopEarly {
                            return Ok(self.refused(EditableTextRefusal::ResidentBytes, shape));
                        }
                        resident_refused = true;
                    }
                } else {
                    prior_break = None;
                    current_line = current_line.saturating_add(1);
                    if current_line > self.max_line_bytes {
                        shape.longest_line_bytes = shape.longest_line_bytes.max(current_line);
                        return Ok(self.refused(EditableTextRefusal::LineBytes, shape));
                    }
                }
            }
        }
        shape.longest_line_bytes = shape.longest_line_bytes.max(current_line);
        let estimate = self.estimate_resident_bytes(shape.source_bytes, shape.logical_lines);
        if resident_refused || estimate > self.max_resident_bytes {
            Ok(self.refused(EditableTextRefusal::ResidentBytes, shape))
        } else {
            Ok(EditableTextDecision::Editable {
                shape,
                estimated_resident_bytes: estimate,
            })
        }
    }

    fn refused(
        self,
        reason: EditableTextRefusal,
        shape: EditableTextShape,
    ) -> EditableTextDecision {
        EditableTextDecision::Paged {
            reason,
            shape,
            estimated_resident_bytes: self
                .estimate_resident_bytes(shape.source_bytes, shape.logical_lines),
        }
    }
}
