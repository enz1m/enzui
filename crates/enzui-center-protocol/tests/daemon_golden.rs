//! Frames and proofs the `enzui-center` daemon itself wrote must read back
//! here and re-encode to the same bytes.
//!
//! `fixtures/daemon-golden.json` was produced by a program linking the
//! daemon's own `framing::write_frame`, protocol types, commit-report types
//! and `crypto::ProofContext`. A drift on either side changes the bytes;
//! regenerate the fixture from the daemon when the wire changes on purpose.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_center_protocol::framing::{encode_to_vec, read_message};
use enzui_center_protocol::protocol::{
    AppToBroker, AppToBrokerV3, Authenticate, AuthenticationResult, AuthenticationResultV3,
    BrokerToApp, BrokerToAppV3, Challenge, CredentialFile,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const GOLDEN: &str = include_str!("fixtures/daemon-golden.json");

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden fixture is JSON")
}

fn round_trip<T: Serialize + DeserializeOwned>(name: &str, bytes: &[u8]) {
    let value: T = read_message(&mut &bytes[..])
        .unwrap_or_else(|error| panic!("{name}: daemon frame refused: {error:?}"));
    let again = encode_to_vec(&value).expect("re-encode");
    assert_eq!(
        String::from_utf8_lossy(&again[4..]),
        String::from_utf8_lossy(&bytes[4..]),
        "{name}: re-encoded JSON differs"
    );
    assert_eq!(again, bytes, "{name}: re-encoded frame differs");
}

#[test]
fn every_daemon_frame_round_trips_byte_for_byte() {
    let golden = golden();
    let frames = golden["frames"].as_array().expect("frames");
    assert_eq!(frames.len(), 23, "fixture lost frames");
    for frame in frames {
        let name = frame["name"].as_str().expect("name");
        let bytes = hex::decode(frame["hex"].as_str().expect("hex")).expect("hex frame");
        match name {
            "challenge" => round_trip::<Challenge>(name, &bytes),
            "authenticate" | "authenticate_v3" => round_trip::<Authenticate>(name, &bytes),
            "auth_accepted" => round_trip::<AuthenticationResult>(name, &bytes),
            "auth_rejected" => {
                round_trip::<AuthenticationResult>(name, &bytes);
                round_trip::<AuthenticationResultV3>(name, &bytes);
            }
            "auth_accepted_v3" => round_trip::<AuthenticationResultV3>(name, &bytes),
            "register" | "event_accepted" | "event_progress" | "event_completed"
            | "event_failed" | "event_cancelled" => round_trip::<AppToBroker>(name, &bytes),
            "registered" | "registration_rejected" | "invoke" | "cancel" => {
                round_trip::<BrokerToApp>(name, &bytes);
                round_trip::<BrokerToAppV3>(name, &bytes);
            }
            "commit_report" => round_trip::<AppToBrokerV3>(name, &bytes),
            ack if ack.starts_with("commit_ack_") => round_trip::<BrokerToAppV3>(name, &bytes),
            other => panic!("unmapped golden frame {other}"),
        }
    }
}

#[test]
fn a_v2_acceptance_is_refused_where_v3_is_expected() {
    // A version-3 reader must refuse a version-2 acceptance (it lacks the
    // negotiated fields) rather than silently treat it as version 3.
    let golden = golden();
    let frame = golden["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .find(|frame| frame["name"] == "auth_accepted")
        .expect("auth_accepted frame");
    let bytes = hex::decode(frame["hex"].as_str().expect("hex")).expect("hex frame");
    assert!(read_message::<_, AuthenticationResultV3>(&mut &bytes[..]).is_err());
}

#[test]
fn the_daemon_credential_file_loads() {
    let golden = golden();
    let credential: CredentialFile =
        serde_json::from_str(golden["credential"].as_str().expect("credential")).expect("parse");
    let expected = hex::decode(golden["proof_context"]["secret"].as_str().expect("secret"))
        .expect("secret hex");
    assert_eq!(credential.app_secret_bytes().expect("app secret"), expected);
}
