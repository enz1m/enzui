use super::test_support::{FakeCancel, FakeClock, FakeMediaReader, FakeResolver, ScriptedProbe};
use super::*;
use crate::url_policy::UrlVerdict;

fn source(url: &str) -> RemoteSource {
    RemoteSource::checked(url, &UrlPolicy::DEFAULT).expect("the test URL passes the string stage")
}

fn run(
    gate: &MediaGate,
    url: &str,
    effects: &GateEffects<'_>,
    reader: &FakeMediaReader,
) -> Result<ApprovedStream, GateRefusal> {
    let outcome = gate.approve(&source(url), effects);
    if let Ok(approved) = &outcome {
        reader.open(approved);
    }
    outcome
}

#[test]
fn a_redirect_chain_past_the_bound_is_refused() {
    let gate = MediaGate {
        policy: UrlPolicy {
            max_redirects: 2,
            ..UrlPolicy::DEFAULT
        },
        ..MediaGate::DEFAULT
    };
    let resolver = FakeResolver::always(&["93.184.216.34"]);
    let probe = ScriptedProbe::new(vec![
        HopReport::Redirect("https://a.example.com/1".to_owned()),
        HopReport::Redirect("https://b.example.com/2".to_owned()),
        HopReport::Redirect("https://c.example.com/3".to_owned()),
        HopReport::Redirect("https://d.example.com/4".to_owned()),
    ]);
    let clock = FakeClock::frozen();
    let cancel = FakeCancel::default();
    let effects = GateEffects {
        resolver: &resolver,
        probe: Some(&probe),
        clock: &clock,
        cancel: &cancel,
    };
    let reader = FakeMediaReader::default();
    let outcome = run(
        &gate,
        "https://cdn.example.com/movie.mp4",
        &effects,
        &reader,
    );
    assert_eq!(
        outcome.err(),
        Some(GateRefusal::Url(UrlVerdict::TooManyRedirects { limit: 2 })),
        "the chain must be bounded, not followed until it ends"
    );
    assert_eq!(reader.opens(), 0);
    assert!(
        probe.calls() <= 3,
        "at most `max_redirects + 1` hops may be contacted, got {}",
        probe.calls()
    );
}
