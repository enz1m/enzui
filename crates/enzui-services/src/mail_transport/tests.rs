use super::*;

#[test]
fn set_mail_from_rejects_nul() {
    let mut easy = curl::easy::Easy::new();
    let err = MailTransport::new(&mut easy)
        .set_mail_from("bad\0addr")
        .unwrap_err();
    assert!(matches!(
        err.kind(),
        MailTransportErrorKind::ControlCharInAddress
    ));
}
