use super::*;
use std::time::Duration;

fn mutation(
    document: VirtualDocumentId,
    revision: VirtualRevision,
    mutation: MutationKind,
    cancellation: &VirtualTextCancellation,
) -> MutationRequest {
    MutationRequest {
        document,
        revision,
        cancellation: cancellation.clone(),
        mutation,
    }
}

// In this race, post-admission cancellation cannot preempt the mutation; the waiter reaches its deadline instead.
#[test]
fn cancellation_after_admission_loses_to_the_reducer() {
    let mailbox = MutationMailbox::with_reply_timeout(1, Duration::from_secs(60));
    let cancellation = VirtualTextCancellation::default();
    let ticket = mailbox
        .enqueue(mutation(
            VirtualDocumentId(7),
            VirtualRevision(11),
            MutationKind::SetSelection(VirtualSelection::default()),
            &cancellation,
        ))
        .unwrap();
    let pending = mailbox.drain().pop().expect("one queued request");
    let (entered, reducer_entered) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let reducer = std::thread::spawn(move || {
        pending.resolve_with(|_| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            MutationReply::Accepted {
                revision: VirtualRevision(12),
                selection: VirtualSelection::default(),
            }
        })
    });
    reducer_entered
        .recv()
        .expect("reducer admitted the request");
    cancellation.cancel();
    release.send(()).unwrap();
    assert!(
        reducer.join().unwrap(),
        "an admitted mutation is delivered despite a later cancellation",
    );
    assert_eq!(
        ticket.wait(),
        Ok(MutationReply::Accepted {
            revision: VirtualRevision(12),
            selection: VirtualSelection::default(),
        }),
        "a post-admission cancel must not fabricate Cancelled for applied bytes",
    );
}
