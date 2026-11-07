use super::*;

#[test]
fn durable_activation_due_claim_wins_while_schedule_publish_is_blocked() {
    let fixture = Fixture::new("daduepublish");
    let platform = Arc::new(FakePlatform::default());
    let gate = Arc::new(BlockGate::default());
    gate.block();
    let scheduler = DurableActivationScheduler::new(
        DesktopApplicationId::new("org.enzui.Calendar").expect("fixture app id"),
        fixture.state.clone(),
        BlockingFacility::new(Arc::clone(&platform)).with_publish_gate(Arc::clone(&gate)),
    );
    let request = fixture.request(
        "due-publish",
        BASE,
        vec!["deliver".into()],
        MissedActivationPolicy::Skip,
    );
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut scheduler = scheduler;
        result_tx
            .send(scheduler.schedule_at(request, UNIX_EPOCH + Duration::from_secs(BASE)))
            .expect("publish schedule result");
    });
    gate.wait_entered();

    let claimer = DurableActivationScheduler::new(
        DesktopApplicationId::new("org.enzui.Calendar").expect("fixture app id"),
        fixture.state.clone(),
        BlockingFacility::new(Arc::clone(&platform)),
    );
    assert!(matches!(
        claimer
            .claim(
                &fixture.invocation("due-publish", 1),
                UNIX_EPOCH + Duration::from_secs(BASE),
            )
            .expect("claim while publish is blocked"),
        ClaimOutcome::Deliver(_)
    ));
    gate.release();
    let result = result_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("receive schedule result");
    worker.join().expect("schedule worker exits");
    assert!(matches!(
        result,
        Err(SchedulerError::Superseded { generation, .. }) if generation.get() == 1
    ));
    assert_eq!(
        claimer.list().expect("delivered custody")[0].status(),
        JobStatus::Delivered
    );
}
