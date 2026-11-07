use crate::download::transfer::*;

#[test]
fn a_cancelled_control_refuses_to_resume() {
    let control = TransferControl::new();
    control.cancel();
    assert!(
        !control.resume(),
        "cancellation is terminal; resuming it would orphan the deleted partial"
    );
    assert_eq!(control.intent(), TransferIntent::Cancel);

    let paused = TransferControl::new();
    paused.pause();
    assert!(paused.resume(), "a pause is resumable");
    assert_eq!(paused.intent(), TransferIntent::Run);
    assert!(
        !paused.signal().is_cancelled(),
        "resuming re-arms the signal, or the next transport call aborts at once"
    );
}
