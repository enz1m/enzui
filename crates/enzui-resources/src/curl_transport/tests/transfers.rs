use super::*;

/// Hostnames resolved by [`scripted_lookup`], chosen so a hop's destination
/// class is a property of its HOST rather than of the policy flags. That is what
/// makes "hop 0 public, hop 1 internal" expressible at all — the thing a
/// `TcpListener` fixture cannot express.
fn scripted_lookup(host: &str, _port: u16) -> Result<Vec<std::net::IpAddr>, ()> {
    let addr = match host {
        // Public (documentation range is classified Special, so use real public
        // addresses here).
        "a.test" => [93, 184, 216, 34],
        "b.test" => [93, 184, 216, 35],
        "c.test" => [93, 184, 216, 36],
        "d.test" => [93, 184, 216, 37],
        "e.test" => [93, 184, 216, 38],
        "f.test" => [93, 184, 216, 39],
        // The SSRF targets a hostile redirect would aim at.
        "internal.test" => [10, 0, 0, 7],
        "metadata.test" => [169, 254, 169, 254],
        "cgnat.test" => [100, 64, 0, 1],
        _ => return Err(()),
    };
    Ok(vec![std::net::IpAddr::from(addr)])
}

/// A wire that reports a peer other than the screened one is refused.
#[test]
fn a_wire_reporting_a_different_peer_than_was_screened_is_refused() {
    /// A wire that ignores its screened peer and reports an internal address,
    /// standing in for a connection that landed somewhere other than the pin.
    struct LyingWire {
        claimed: std::net::IpAddr,
    }
    impl Wire for LyingWire {
        fn transfer(
            &self,
            _exchange: &super::WireExchange<'_>,
            _progress: &mut dyn FnMut(TransferProgress),
        ) -> Result<WireOutcome, TransportError> {
            Ok(WireOutcome {
                status: 200,
                body: b"REBOUND".to_vec(),
                headers: Vec::new(),
                primary: self.claimed,
            })
        }
    }

    let policy = screening_policy(5);
    // A DIFFERENT but still PUBLIC address: `check_address` allows it, so only
    // the pin comparison can refuse this. See the doc comment.
    let liar = LyingWire {
        claimed: std::net::IpAddr::from([8, 8, 8, 8]),
    };
    let error = super::fetch_with_headers(
        &request("https://a.test/x", policy),
        super::HopContext {
            policy: &policy,
            limits: &TransportLimits::DEFAULT,
            extra_headers: &[],
        },
        &super::CancelProbe::signal_only(&CancelSignal::new()),
        &mut |_| {},
        super::HopEffects {
            lookup: Arc::new(scripted_lookup),
            wire: &liar,
        },
    )
    .expect_err("a peer that is not the screened one must be refused");
    assert_eq!(
        error,
        TransportError::AddressBlocked(AddressClass::Public),
        "a peer that differs from the screened one must be refused EVEN WHEN its \
         class is allowable: 8.8.8.8 passes `check_address`, so a pass here means \
         the pin comparison is the only thing that refused it. The class reported \
         is that of the address actually connected to"
    );

    // Positive control: a wire reporting the peer it was GIVEN succeeds, so the
    // test above is discriminating rather than refusing every wire.
    let honest = LyingWire {
        claimed: std::net::IpAddr::from([93, 184, 216, 34]),
    };
    let response = super::fetch_with_headers(
        &request("https://a.test/x", policy),
        super::HopContext {
            policy: &policy,
            limits: &TransportLimits::DEFAULT,
            extra_headers: &[],
        },
        &super::CancelProbe::signal_only(&CancelSignal::new()),
        &mut |_| {},
        super::HopEffects {
            lookup: Arc::new(scripted_lookup),
            wire: &honest,
        },
    )
    .expect("a wire reporting the screened peer must succeed");
    assert_eq!(response.body, b"REBOUND");
    assert_eq!(response.peer_class, AddressClass::Public);
}
