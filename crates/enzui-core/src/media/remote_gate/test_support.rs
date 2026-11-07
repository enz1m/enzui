#![allow(missing_docs, reason = "test fixtures: each name says what it fakes")]

use std::cell::{Cell, RefCell};
use std::net::IpAddr;

use super::{
    Clock, EffectFailed, GateEffects, HopReport, HostResolver, RedirectProbe, StreamBudget,
};

type ScriptedAnswer = (String, Result<Vec<IpAddr>, EffectFailed>);
use crate::url_policy::TransportLimits;

#[derive(Debug, Default)]
pub struct FakeResolver {
    answers: RefCell<Vec<ScriptedAnswer>>,
    default: RefCell<Option<Result<Vec<IpAddr>, EffectFailed>>>,
    calls: Cell<u32>,
}

impl FakeResolver {
    #[must_use]
    pub fn always(addresses: &[&str]) -> Self {
        let resolver = FakeResolver::default();
        *resolver.default.borrow_mut() = Some(Ok(parse_all(addresses)));
        resolver
    }

    #[must_use]
    pub fn failing() -> Self {
        let resolver = FakeResolver::default();
        *resolver.default.borrow_mut() = Some(Err(EffectFailed::Unavailable));
        resolver
    }

    #[must_use]
    pub fn with(self, host: &str, addresses: &[&str]) -> Self {
        self.answers
            .borrow_mut()
            .push((host.to_owned(), Ok(parse_all(addresses))));
        self
    }

    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.get()
    }
}

impl HostResolver for FakeResolver {
    fn resolve(
        &self,
        host: &str,
        _port: u16,
        _limits: &TransportLimits,
        _clock: &dyn Clock,
        _cancel: &dyn super::CancelView,
    ) -> Result<Vec<IpAddr>, EffectFailed> {
        self.calls.set(self.calls.get() + 1);
        for (name, answer) in self.answers.borrow().iter().rev() {
            if name == host {
                return answer.clone();
            }
        }
        self.default
            .borrow()
            .clone()
            .unwrap_or(Err(EffectFailed::Unavailable))
    }
}

fn parse_all(addresses: &[&str]) -> Vec<IpAddr> {
    addresses
        .iter()
        .map(|a| a.parse::<IpAddr>().expect("a test address must parse"))
        .collect()
}

#[derive(Debug, Default)]
pub struct ScriptedProbe {
    hops: RefCell<Vec<HopReport>>,
    calls: Cell<usize>,
    fail: Cell<bool>,
    peers: RefCell<Vec<IpAddr>>,
    urls: RefCell<Vec<String>>,
}

impl ScriptedProbe {
    #[must_use]
    pub fn new(hops: Vec<HopReport>) -> Self {
        ScriptedProbe {
            hops: RefCell::new(hops),
            ..ScriptedProbe::default()
        }
    }

    #[must_use]
    pub fn terminal() -> Self {
        ScriptedProbe::new(Vec::new())
    }

    #[must_use]
    pub fn failing() -> Self {
        let probe = ScriptedProbe::terminal();
        probe.fail.set(true);
        probe
    }

    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls.get()
    }

    #[must_use]
    pub fn peers(&self) -> Vec<IpAddr> {
        self.peers.borrow().clone()
    }

    #[must_use]
    pub fn urls(&self) -> Vec<String> {
        self.urls.borrow().clone()
    }
}

impl RedirectProbe for ScriptedProbe {
    fn probe(
        &self,
        url: &str,
        endpoint: std::net::SocketAddr,
        _limits: &TransportLimits,
        _budget: &StreamBudget,
        _clock: &dyn Clock,
        _cancel: &dyn super::CancelView,
    ) -> Result<HopReport, EffectFailed> {
        self.calls.set(self.calls.get() + 1);
        self.peers.borrow_mut().push(endpoint.ip());
        self.urls.borrow_mut().push(url.to_owned());
        if self.fail.get() {
            return Err(EffectFailed::Unavailable);
        }
        let mut hops = self.hops.borrow_mut();
        if hops.is_empty() {
            Ok(HopReport::Final)
        } else {
            Ok(hops.remove(0))
        }
    }
}

#[derive(Debug, Default)]
pub struct FakeClock {
    now: Cell<u64>,
    step: Cell<u64>,
}

impl FakeClock {
    #[must_use]
    pub fn frozen() -> Self {
        FakeClock::default()
    }

    #[must_use]
    pub fn ticking(step: u64) -> Self {
        let clock = FakeClock::frozen();
        clock.step.set(step);
        clock
    }

    pub fn set(&self, now_ms: u64) {
        self.now.set(now_ms);
    }
}

impl Clock for FakeClock {
    fn elapsed_ms(&self) -> u64 {
        let now = self.now.get();
        self.now.set(now.saturating_add(self.step.get()));
        now
    }
}

#[derive(Debug, Default)]
pub struct FakeCancel {
    cancelled: Cell<bool>,
}

impl FakeCancel {
    pub fn cancel(&self) {
        self.cancelled.set(true);
    }
}

impl super::CancelView for FakeCancel {
    fn is_cancelled(&self) -> bool {
        self.cancelled.get()
    }
}

#[derive(Debug, Default)]
pub struct FakeMediaReader {
    opens: Cell<u32>,
    bytes: Cell<u64>,
}

impl FakeMediaReader {
    pub fn open(&self, approved: &super::ApprovedStream) {
        self.opens.set(self.opens.get() + 1);
        self.bytes
            .set(self.bytes.get() + approved.budget().buffer_bytes.min(4096));
    }

    #[must_use]
    pub fn opens(&self) -> u32 {
        self.opens.get()
    }

    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes.get()
    }
}

pub struct PublicEffects {
    pub resolver: FakeResolver,
    pub probe: ScriptedProbe,
    pub clock: FakeClock,
    pub cancel: FakeCancel,
}

impl PublicEffects {
    #[must_use]
    pub fn effects(&self) -> GateEffects<'_> {
        GateEffects {
            resolver: &self.resolver,
            probe: Some(&self.probe),
            clock: &self.clock,
            cancel: &self.cancel,
        }
    }
}

#[must_use]
pub fn public_effects() -> PublicEffects {
    PublicEffects {
        resolver: FakeResolver::always(&["93.184.216.34"]),
        probe: ScriptedProbe::terminal(),
        clock: FakeClock::frozen(),
        cancel: FakeCancel::default(),
    }
}
