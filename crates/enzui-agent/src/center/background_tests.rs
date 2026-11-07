use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
#[derive(Debug, Clone)]
enum Msg {
    Task(Result<u32, TaskHostError>),
    Start(u32),
}

/// A small application: `Start(n)` submits a task that answers `n` after a
/// delay; the app is Working while any is outstanding and presents when it has
/// received `present_after` answers.
struct Fixture {
    tasks: Arc<TaskHost<Msg>>,
    outstanding: usize,
    answers: Vec<u32>,
    refusals: Vec<TaskHostError>,
    present_after: Option<usize>,
    wake_at: Option<Instant>,
    wakes: usize,
    turns: Arc<AtomicUsize>,
    delay: Duration,
}

impl std::fmt::Debug for Fixture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fixture")
            .field("outstanding", &self.outstanding)
            .field("answers", &self.answers)
            .field("refusals", &self.refusals)
            .finish_non_exhaustive()
    }
}

impl Fixture {
    fn new(tasks: Arc<TaskHost<Msg>>) -> Self {
        Self {
            tasks,
            outstanding: 0,
            answers: Vec::new(),
            refusals: Vec::new(),
            present_after: None,
            wake_at: None,
            wakes: 0,
            turns: Arc::new(AtomicUsize::new(0)),
            delay: Duration::from_millis(20),
        }
    }

    fn answer_later(&mut self, value: u32) -> Task<Msg> {
        self.outstanding += 1;
        let delay = self.delay;
        Task::perform(
            async move {
                tokio::time::sleep(delay).await;
                value
            },
            |value| Msg::Task(Ok(value)),
        )
    }
}

impl BackgroundApp for Fixture {
    type Message = Msg;

    fn update(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::Start(value) => self.answer_later(value),
            Msg::Task(Ok(value)) => {
                self.outstanding -= 1;
                self.answers.push(value);
                Task::none()
            }
            Msg::Task(Err(error)) => {
                self.refusals.push(error);
                Task::none()
            }
        }
    }

    fn subscription(&self) -> Subscription<Msg> {
        self.tasks.subscription(1, |delivered| match delivered {
            Ok(message) => message,
            Err(error) => Msg::Task(Err(error)),
        })
    }

    fn disposition(&self) -> Disposition {
        self.turns.fetch_add(1, Ordering::AcqRel);
        if self
            .present_after
            .is_some_and(|count| self.answers.len() >= count)
        {
            Disposition::Present
        } else if self.outstanding > 0 || self.tasks.has_work() {
            Disposition::Working
        } else {
            Disposition::Idle
        }
    }

    fn wake_at(&self) -> Option<Instant> {
        self.wake_at
    }

    fn wake(&mut self) -> Task<Msg> {
        self.wakes += 1;
        self.wake_at = None;
        Task::none()
    }

    fn refused(&mut self, error: TaskHostError) -> Task<Msg> {
        self.outstanding = self.outstanding.saturating_sub(1);
        self.refusals.push(error);
        Task::none()
    }
}

#[test]
fn a_refused_task_is_reported_to_the_app_instead_of_disappearing() {
    let tasks = Arc::new(TaskHost::spawn().unwrap());
    let mut app = Fixture::new(Arc::clone(&tasks));
    app.delay = Duration::from_millis(300);
    // Occupy every slot directly, so the runner's own submission is refused.
    for value in 0..super::super::task_host::TASK_CAPACITY as u32 {
        let task = app.answer_later(value);
        tasks.submit(task).unwrap();
    }
    let boot = app.answer_later(999);

    let exit = run(app, boot, &tasks, Duration::from_millis(50)).unwrap();

    let BackgroundExit::Idle(app) = exit else {
        panic!("{exit:?}");
    };
    assert_eq!(
        app.refusals,
        vec![TaskHostError::Full],
        "the runner must report the refused boot task"
    );
    assert!(!app.answers.contains(&999), "a refused task never ran");
    assert_eq!(
        app.answers.len(),
        super::super::task_host::TASK_CAPACITY,
        "the accepted work still completed"
    );
}

/// Launches waiting in a shared queue, as the single-instance wake keeps
/// them; the subscription takes every one that is there.
static LAUNCHES: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// Presents on its first launch and records every launch it handles.
#[derive(Debug, Default)]
struct Launcher {
    launches: Vec<u32>,
}

impl BackgroundApp for Launcher {
    type Message = u32;

    fn update(&mut self, launch: u32) -> Task<u32> {
        self.launches.push(launch);
        Task::none()
    }

    fn subscription(&self) -> Subscription<u32> {
        Subscription::run(|| {
            let queued = std::mem::take(&mut *LAUNCHES.lock().unwrap());
            iced_futures::futures::stream::iter(queued)
        })
    }

    fn disposition(&self) -> Disposition {
        if self.launches.is_empty() {
            Disposition::Working
        } else {
            Disposition::Present
        }
    }

    fn wake_at(&self) -> Option<Instant> {
        None
    }

    fn wake(&mut self) -> Task<u32> {
        Task::none()
    }

    fn refused(&mut self, _: TaskHostError) -> Task<u32> {
        Task::none()
    }
}

/// A second launch the subscription took from its queue in the same turn as
/// the one that opens the window is handled, not dropped with the loop.
#[test]
fn a_message_taken_with_the_one_that_presents_is_not_lost() {
    LAUNCHES.lock().unwrap().extend([1, 2]);
    let tasks = TaskHost::spawn().unwrap();
    let exit = run(
        Launcher::default(),
        Task::none(),
        &tasks,
        Duration::from_secs(5),
    )
    .unwrap();
    let BackgroundExit::Present(app) = exit else {
        panic!("{exit:?}");
    };
    assert_eq!(app.launches, vec![1, 2]);
    assert!(LAUNCHES.lock().unwrap().is_empty(), "both left the queue");
}
