// Accept only the RFC 9110 opaque-tag byte set, including an empty quoted tag.
fn is_valid_opaque_tag(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 2 || b[0] != b'"' || b[b.len() - 1] != b'"' {
        return false;
    }
    b[1..b.len() - 1]
        .iter()
        .all(|&c| c == 0x21 || (0x23..=0x7E).contains(&c) || c >= 0x80)
}

// Validate IMF-fixdate shape because validators are compared byte-for-byte, not interpreted as calendar dates.
fn is_valid_http_date(s: &str) -> bool {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let b = s.as_bytes();
    // Compare HTTP-date anchors as bytes so non-ASCII input cannot panic at a UTF-8 slice boundary.
    if b.len() != 29 {
        return false;
    }
    if !DAYS.iter().any(|d| d.as_bytes() == &b[0..3]) || &b[3..5] != b", " {
        return false;
    }
    if !MONTHS.iter().any(|m| m.as_bytes() == &b[8..11]) {
        return false;
    }
    if b[7] != b' ' || b[11] != b' ' || b[16] != b' ' || b[25] != b' ' {
        return false;
    }
    if b[19] != b':' || b[22] != b':' {
        return false;
    }
    if &b[26..29] != b"GMT" {
        return false;
    }
    for r in [5..7, 12..16, 17..19, 20..22, 23..25] {
        if !b[r].iter().all(u8::is_ascii_digit) {
            return false;
        }
    }
    true
}

/// A stored HTTP validator used to determine whether a partial can be resumed.
///
/// Only syntactically valid strong ETags and HTTP dates can authorize `If-Range`; weak ETags remain representable but cannot authorize a resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validator {
    /// A candidate strong ETag, including its quotes; syntax is checked before `If-Range` use.
    StrongEtag(String),
    /// A candidate weak ETag, which is validated before use and never accepted for byte-range `If-Range`.
    WeakEtag(String),
    /// A candidate `Last-Modified` value; syntax is checked before `If-Range` use.
    LastModified(String),
}

impl Validator {
    /// Parses an ETag using RFC 9110 entity-tag grammar.
    #[must_use]
    pub fn parse_etag(header: &str) -> Option<Validator> {
        if let Some(rest) = header.strip_prefix("W/") {
            is_valid_opaque_tag(rest).then(|| Validator::WeakEtag(header.to_owned()))
        } else {
            is_valid_opaque_tag(header).then(|| Validator::StrongEtag(header.to_owned()))
        }
    }

    /// Builds a strong ETag validator, returning `None` for malformed or weak tags.
    #[must_use]
    pub fn strong_etag(header: &str) -> Option<Validator> {
        match Validator::parse_etag(header) {
            Some(Validator::StrongEtag(v)) => Some(Validator::StrongEtag(v)),
            _ => None,
        }
    }

    /// Build a `Last-Modified` validator, rejecting a malformed HTTP-date.
    #[must_use]
    pub fn last_modified(header: &str) -> Option<Validator> {
        is_valid_http_date(header).then(|| Validator::LastModified(header.to_owned()))
    }

    /// Reports whether the stored validator text satisfies its RFC grammar, including for directly constructed variants.
    #[must_use]
    pub fn is_syntactically_valid(&self) -> bool {
        match self {
            Validator::StrongEtag(v) => is_valid_opaque_tag(v),
            Validator::WeakEtag(v) => v.strip_prefix("W/").is_some_and(is_valid_opaque_tag),
            Validator::LastModified(v) => is_valid_http_date(v),
        }
    }

    /// Reports whether the validator is syntactically valid and strong enough for `If-Range`.
    #[must_use]
    pub fn is_strong(&self) -> bool {
        !matches!(self, Validator::WeakEtag(_)) && self.is_syntactically_valid()
    }

    /// The header value to send in `If-Range`, or `None` when this validator
    /// may not be used for one.
    #[must_use]
    pub fn if_range_value(&self) -> Option<&str> {
        match self {
            Validator::StrongEtag(v) | Validator::LastModified(v) if self.is_strong() => Some(v),
            _ => None,
        }
    }
}

/// A validated byte-range request plan.
///
/// Conditional plans can contain only a syntactically valid strong validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeRequestPlan {
    offset: u64,
    validator: Option<Validator>,
}

impl RangeRequestPlan {
    /// Builds a conditional range request, returning `None` unless the validator is syntactically valid and strong.
    #[must_use]
    pub fn conditional(offset: u64, validator: Validator) -> Option<Self> {
        validator.is_strong().then_some(Self {
            offset,
            validator: Some(validator),
        })
    }

    /// Builds an unconditional range request, which cannot authorize appending a `206` response.
    #[must_use]
    pub fn unconditional(offset: u64) -> Self {
        Self {
            offset,
            validator: None,
        }
    }

    /// The requested byte offset.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The validated strong validator, if this is a conditional plan.
    #[must_use]
    pub fn validator(&self) -> Option<&Validator> {
        self.validator.as_ref()
    }

    /// Reports whether this plan carries a strong validator and can authorize resumption.
    #[must_use]
    pub fn is_conditional(&self) -> bool {
        self.validator.is_some()
    }

    /// Returns the `Range` and optional `If-Range` header values.
    #[must_use]
    pub fn headers(&self) -> (String, Option<String>) {
        (
            format!("bytes={}-", self.offset),
            self.validator
                .as_ref()
                .and_then(|v| v.if_range_value().map(str::to_owned)),
        )
    }
}

/// The content coding used to interpret byte-range offsets.
///
/// A resumed response must use the same coding as the partial file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentCoding {
    /// No coding applied (`Content-Encoding` absent, or `identity`).
    Identity,
    /// A non-identity content-coding token.
    Coded(String),
}

impl ContentCoding {
    /// Interprets an optional `Content-Encoding`; absent or case-insensitive `identity` means no coding.
    #[must_use]
    pub fn from_header(header: Option<&str>) -> ContentCoding {
        match header {
            None => ContentCoding::Identity,
            Some(h) if h.eq_ignore_ascii_case("identity") => ContentCoding::Identity,
            Some(h) => ContentCoding::Coded(h.to_ascii_lowercase()),
        }
    }
}

/// Returns range-resumption header values, using `If-Range` only for a valid strong validator.
#[must_use]
pub fn resume_headers(
    resume_offset: u64,
    validator: Option<&Validator>,
) -> (String, Option<String>) {
    let plan = validator
        .cloned()
        .and_then(|v| RangeRequestPlan::conditional(resume_offset, v))
        .unwrap_or_else(|| RangeRequestPlan::unconditional(resume_offset));
    plan.headers()
}

/// Components of a `Content-Range` response header.
///
/// [`parse_content_range`] rejects malformed forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRange {
    /// The first byte position in the range (inclusive).
    pub start: u64,
    /// The last byte position in the range (inclusive).
    pub end: u64,
    /// The total size of the resource, or `None` when the server sent `*`.
    pub total: Option<u64>,
}

impl ContentRange {
    /// Reports whether this hand-built range has no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.end < self.start
    }

    /// Returns the inclusive range length, or `None` for a reversed range or arithmetic overflow.
    #[must_use]
    pub fn len(&self) -> Option<u64> {
        if self.end < self.start {
            return None;
        }
        self.end
            .checked_sub(self.start)
            .and_then(|d| d.checked_add(1))
    }

    /// Returns the offset after the inclusive end, or `None` when it is not representable.
    #[must_use]
    pub fn end_exclusive(&self) -> Option<u64> {
        self.end.checked_add(1)
    }
}

/// Parses a single-byte-range `Content-Range` using RFC 9110 grammar and validity rules.
///
/// The parser rejects nonrepresentable end-exclusive offsets; use [`parse_unsatisfied_range`] for `bytes */N`.
#[must_use]
pub fn parse_content_range(header: &str) -> Option<ContentRange> {
    // Accept only the `bytes` unit followed by exactly one space; unknown units cannot be recombined.
    let rest = header.strip_prefix("bytes ")?;

    let (range_part, total_part) = rest.split_once('/')?;
    let (start_s, end_s) = range_part.split_once('-')?;

    let start = parse_digits(start_s)?;
    let end = parse_digits(end_s)?;

    if end < start {
        return None;
    }

    let end_exclusive = end.checked_add(1)?;

    let total = if total_part == "*" {
        None
    } else {
        let t = parse_digits(total_part)?;
        // A complete length must exceed the inclusive end, so compare it with `end + 1`.
        if t < end_exclusive {
            return None;
        }
        Some(t)
    };

    Some(ContentRange { start, end, total })
}

// Require ASCII digits before parsing because `u64::from_str` accepts a leading plus sign.
fn parse_digits(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse::<u64>().ok()
}

/// Parses `bytes */N` and returns the current complete length.
#[must_use]
pub fn parse_unsatisfied_range(header: &str) -> Option<u64> {
    parse_digits(header.strip_prefix("bytes */")?)
}

/// Specifies whether the held response body is written or discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyDisposition {
    /// Writes the held body; the enclosing decision supplies its offset and truncation behavior.
    ConsumeAsFullBody,
    /// Throw the body away without writing a byte of it.
    Discard,
}

/// Describes how a range response affects the partial file and body in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeDecision {
    /// Append the response body to the existing partial file, starting at
    /// `append_offset`.
    Resume {
        /// Where to start appending (equals the `Content-Range` `first-pos`).
        append_offset: u64,
        /// The total size of the resource, if the server announced one.
        /// Feed this to [`Download::set_total`](crate::download::Download::set_total); `None` means the server sent
        /// `*` and the length stays unknown.
        total: Option<u64>,
        /// The byte contract that must hold before an appended body is complete.
        expected: ExpectedBody,
    },
    /// Truncates the partial and consumes the held body as the full representation.
    RestartWithBody {
        /// Why the restart was chosen.
        reason: RangeRestartReason,
        /// The full-body disposition for this restart.
        disposition: BodyDisposition,
        /// The contract the full body must satisfy, when the server
        /// announced a length.
        expected: ExpectedBody,
    },
    /// Discards the held body without modifying the partial and requests a fresh attempt.
    DiscardAndRetry {
        /// Why the body was rejected.
        reason: RangeRetryReason,
        /// Disposition selected for the held body.
        disposition: BodyDisposition,
    },
    /// Discards the held body without modifying the partial because the response is malformed or unsupported.
    Refuse {
        /// Why.
        reason: RangeRefuseReason,
    },
}

impl RangeDecision {
    /// Returns the required disposition of the held response body.
    #[must_use]
    pub fn body_disposition(&self) -> BodyDisposition {
        match self {
            RangeDecision::Resume { .. } => BodyDisposition::ConsumeAsFullBody,
            RangeDecision::RestartWithBody { disposition, .. }
            | RangeDecision::DiscardAndRetry { disposition, .. } => *disposition,
            RangeDecision::Refuse { .. } => BodyDisposition::Discard,
        }
    }

    /// Whether this decision writes any of the held body to the file.
    #[must_use]
    pub fn writes_body(&self) -> bool {
        matches!(
            self,
            RangeDecision::Resume { .. } | RangeDecision::RestartWithBody { .. }
        )
    }

    /// Whether this decision truncates the partial file before writing.
    #[must_use]
    pub fn truncates(&self) -> bool {
        matches!(self, RangeDecision::RestartWithBody { .. })
    }

    /// The body contract to check received bytes against, when there is one.
    #[must_use]
    pub fn expected_body(&self) -> Option<&ExpectedBody> {
        match self {
            RangeDecision::Resume { expected, .. }
            | RangeDecision::RestartWithBody { expected, .. } => Some(expected),
            RangeDecision::DiscardAndRetry { .. } | RangeDecision::Refuse { .. } => None,
        }
    }
}

/// Why a restart-consuming-this-body was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeRestartReason {
    /// The server returned the full representation, which must replace rather than append to the partial.
    ServerSentFullBody,
}

/// Why the body in flight must be discarded and the request retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeRetryReason {
    /// The response validator identifies a different representation, so its suffix cannot be appended.
    ValidatorMismatch,
    /// An unconditional `206` cannot prove that its suffix belongs to the partial.
    UnvalidatedResume,
    /// The response starts at a different offset than requested.
    StartMismatch {
        /// Requested byte offset.
        expected: u64,
        /// What the server sent.
        got: u64,
    },
    /// The response complete length differs from the stored total, indicating a changed representation.
    TotalMismatch {
        /// Stored resource total.
        stored: u64,
        /// What the server's `Content-Range` claims.
        server: u64,
    },
    /// The partial offset exceeds the current resource length; the `416` body is discarded as error content.
    RangeNotSatisfiable {
        /// Current resource length from `bytes */N`, when the server provides it.
        complete_length: Option<u64>,
    },
}

/// Why a range response was refused as malformed or unsupported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeRefuseReason {
    /// A single-part `206` response omitted the required `Content-Range` header.
    MissingContentRange,
    /// The value violates RFC 9110 `Content-Range` grammar or bounds, or its end-exclusive offset does not fit in `u64`.
    MalformedContentRange,
    /// The response's validator is syntactically invalid, so it cannot be
    /// compared with the stored one at all (RFC 9110 §8.8.3 grammar).
    MalformedValidator,
    /// The response coding differs from the partial coding, making its byte offsets incompatible.
    CodingMismatch,
    /// A declared body length contradicts the `Content-Range` it accompanies.
    LengthContradiction {
        /// Bytes the `Content-Range` describes.
        from_range: u64,
        /// Bytes `Content-Length` declares.
        from_length: u64,
    },
    /// An HTTP status this module does not model for range resumption.
    UnexpectedStatus(u16),
}

/// The byte-count and coding contract required before a response can complete a transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedBody {
    /// The exact number of body octets expected, or `None` when no length was declared.
    pub len: Option<u64>,
    /// Expected destination offset after writing the body, when known.
    pub end_exclusive: Option<u64>,
    /// The coding the octets are carried under — and therefore the coding the
    /// partial file must already be in for an append to be valid.
    pub coding: ContentCoding,
}

/// The outcome of checking received bytes against an [`ExpectedBody`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyCheck {
    /// The byte count matches the contract exactly. Safe to promote.
    Exact,
    /// Fewer bytes arrived than declared, so the partial cannot be completed.
    Short {
        /// How many bytes were expected.
        expected: u64,
        /// How many arrived.
        got: u64,
    },
    /// More bytes arrived than declared, so the written body exceeds its byte contract.
    Overlong {
        /// How many bytes were expected.
        expected: u64,
        /// How many arrived.
        got: u64,
    },
    /// No length was declared, so the body cannot be verified.
    Unverifiable,
}

impl BodyCheck {
    /// Reports whether the declared and received byte counts match exactly.
    #[must_use]
    pub fn is_ok(self) -> bool {
        matches!(self, BodyCheck::Exact)
    }
}

impl ExpectedBody {
    /// A contract with nothing declared — no length, no endpoint, identity
    /// coding. [`ExpectedBody::check`] on this yields
    /// [`BodyCheck::Unverifiable`].
    #[must_use]
    pub fn unknown() -> Self {
        Self {
            len: None,
            end_exclusive: None,
            coding: ContentCoding::Identity,
        }
    }

    /// Checks the number of coded octets received against this contract.
    #[must_use]
    pub fn check(&self, received_len: u64) -> BodyCheck {
        match self.len {
            None => BodyCheck::Unverifiable,
            Some(expected) if received_len == expected => BodyCheck::Exact,
            Some(expected) if received_len < expected => BodyCheck::Short {
                expected,
                got: received_len,
            },
            Some(expected) => BodyCheck::Overlong {
                expected,
                got: received_len,
            },
        }
    }
}

/// Protocol metadata from a server response to a range request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RangeResponse<'a> {
    /// The HTTP status code (e.g. 200, 206, 416).
    pub status: u16,
    /// The `Content-Range` header value, if present.
    pub content_range: Option<&'a str>,
    /// The server's validator from the response (`ETag` or `Last-Modified`),
    /// if present.
    pub response_validator: Option<&'a Validator>,
    /// The response-body length, which for `206` is the part length rather than the representation length.
    pub content_length: Option<u64>,
    /// The `Content-Encoding` header value, if present. `None` means
    /// identity.
    pub content_encoding: Option<&'a str>,
}

/// Determines whether to resume, replace, discard and retry, or refuse a range response.
///
/// Malformed metadata is refused before arithmetic. Appending requires a valid `206` for a conditional plan, and any response validator that is present must match the plan.
#[must_use]
pub fn decide_range_response(
    plan: &RangeRequestPlan,
    stored_total: Option<u64>,
    partial_coding: &ContentCoding,
    response: &RangeResponse<'_>,
) -> RangeDecision {
    let response_coding = ContentCoding::from_header(response.content_encoding);

    match response.status {
        // A `200` body is the full representation and must replace, not append to, the partial.
        200 => RangeDecision::RestartWithBody {
            reason: RangeRestartReason::ServerSentFullBody,
            disposition: BodyDisposition::ConsumeAsFullBody,
            expected: ExpectedBody {
                len: response.content_length,
                end_exclusive: response.content_length,
                coding: response_coding,
            },
        },

        206 => {
            let Some(cr_header) = response.content_range else {
                return RangeDecision::Refuse {
                    reason: RangeRefuseReason::MissingContentRange,
                };
            };
            let Some(cr) = parse_content_range(cr_header) else {
                return RangeDecision::Refuse {
                    reason: RangeRefuseReason::MalformedContentRange,
                };
            };
            // Defend the arithmetic boundary even though the parser currently guarantees both values.
            let (Some(range_len), Some(end_exclusive)) = (cr.len(), cr.end_exclusive()) else {
                return RangeDecision::Refuse {
                    reason: RangeRefuseReason::MalformedContentRange,
                };
            };

            if let Some(v) = response.response_validator
                && !v.is_syntactically_valid()
            {
                return RangeDecision::Refuse {
                    reason: RangeRefuseReason::MalformedValidator,
                };
            }

            // Coding must match the partial's: ranges are counted over encoded
            // octets (RFC 9110 §14.1.2), so a coding change invalidates every
            // offset even when the numbers agree.
            if response_coding != *partial_coding {
                return RangeDecision::Refuse {
                    reason: RangeRefuseReason::CodingMismatch,
                };
            }

            if let Some(declared) = response.content_length
                && declared != range_len
            {
                return RangeDecision::Refuse {
                    reason: RangeRefuseReason::LengthContradiction {
                        from_range: range_len,
                        from_length: declared,
                    },
                };
            }

            if cr.start != plan.offset() {
                return RangeDecision::DiscardAndRetry {
                    reason: RangeRetryReason::StartMismatch {
                        expected: plan.offset(),
                        got: cr.start,
                    },
                    disposition: BodyDisposition::Discard,
                };
            }

            if let (Some(stored), Some(server)) = (stored_total, cr.total)
                && stored != server
            {
                return RangeDecision::DiscardAndRetry {
                    reason: RangeRetryReason::TotalMismatch { stored, server },
                    disposition: BodyDisposition::Discard,
                };
            }

            // Appending requires proof that the on-disk prefix and response suffix are the same representation.
            let Some(plan_validator) = plan.validator() else {
                return RangeDecision::DiscardAndRetry {
                    reason: RangeRetryReason::UnvalidatedResume,
                    disposition: BodyDisposition::Discard,
                };
            };
            if let Some(resp_v) = response.response_validator
                && plan_validator != resp_v
            {
                return RangeDecision::DiscardAndRetry {
                    reason: RangeRetryReason::ValidatorMismatch,
                    disposition: BodyDisposition::Discard,
                };
            }

            RangeDecision::Resume {
                append_offset: cr.start,
                total: cr.total,
                expected: ExpectedBody {
                    len: Some(range_len),
                    end_exclusive: Some(end_exclusive),
                    coding: response_coding,
                },
            }
        }

        // A `416` body is not file content; discard it and retain any reported complete length.
        416 => RangeDecision::DiscardAndRetry {
            reason: RangeRetryReason::RangeNotSatisfiable {
                complete_length: response.content_range.and_then(parse_unsatisfied_range),
            },
            disposition: BodyDisposition::Discard,
        },

        other => RangeDecision::Refuse {
            reason: RangeRefuseReason::UnexpectedStatus(other),
        },
    }
}
