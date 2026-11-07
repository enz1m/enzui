use super::tests::{app_credential, config_for, listening_socket};
use super::*;
use crate::center::protocol::{Authenticate, Challenge};
use std::os::unix::net::UnixListener;

fn accept(listener: &UnixListener) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "expected connection did not arrive"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("accept: {error}"),
        }
    }
}

fn request(stream: &mut UnixStream, uid: u32, version: u32) -> Authenticate {
    wire::write_message(
        stream,
        &Challenge {
            version: 2,
            nonce: "00".repeat(32),
        },
    )
    .unwrap();
    let request: Authenticate = wire::read_message(stream).unwrap();
    assert_eq!(request.version, version);
    assert_eq!(request.role, WireRole::App);
    assert_eq!(request.credential_id, "org.enzui.Example");
    assert!(auth::valid_nonce(&request.client_nonce));
    let context = auth::ProofContext {
        server_nonce: &"00".repeat(32),
        client_nonce: &request.client_nonce,
        role: request.role,
        credential_id: &request.credential_id,
        uid,
    };
    let proof = if version == 3 {
        context.v3_client_proof(&[7; 32])
    } else {
        context.proof(&[7; 32], auth::DIRECTION_CLIENT)
    };
    assert_eq!(request.proof, proof);
    request
}

#[test]
fn negotiation_second_legacy_closure_does_not_open_a_third_connection() {
    let dir = crate::center::tests_support::scratch_dir("negotiate-once");
    let (listener, path) = listening_socket(&dir.join("run"), 0o700);
    let config = config_for(path, &dir);
    let uid = runtime_dir::current_uid().unwrap();
    let shared = Shared::new().unwrap();
    let broker = std::thread::spawn(move || {
        for version in [3, 2] {
            let mut stream = accept(&listener);
            request(&mut stream, uid, version);
        }
        listener
    });
    let result = connect_and_register(&config, &app_credential(), uid, &shared);
    assert!(matches!(result, Err(SetupError::Transient(_))));
    assert!(shared.active_stream.lock().unwrap().is_none());
    let listener = broker.join().unwrap();
    assert!(matches!(listener.accept(), Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
}
