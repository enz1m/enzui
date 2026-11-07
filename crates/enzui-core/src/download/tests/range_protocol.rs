use super::*;

#[test]
fn a_206_without_content_range_is_refused() {
    let decision = decide_range_response(
        &plan_at(4096),
        Some(10_000),
        &ContentCoding::Identity,
        &RangeResponse {
            status: 206,
            content_range: None,
            response_validator: Some(&etag_a()),
            content_length: None,
            content_encoding: None,
        },
    );
    assert_eq!(
        decision,
        RangeDecision::Refuse {
            reason: RangeRefuseReason::MissingContentRange,
        }
    );
}
