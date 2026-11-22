use super::{FramePresenter, ReadyFrame};
use enzui_core::media::Rgba8Frame;
use iced::Task;

/// One active image allocation and the newest pending frame.
/// Clear on source replacement; retired uploads retain admission until completion.
/// Deliver every returned task result, including after clear. Dropped or never
/// completed tasks keep admission occupied. Frame generations are caller-owned.
#[derive(Debug, Default)]
pub struct LatestFramePresenter {
    presenter: FramePresenter,
    in_flight: Option<u64>,
    pending: Option<Rgba8Frame>,
}

/// Whether a completion replaced the shown image and which pending allocation to run next.
#[derive(Debug)]
#[must_use = "run any next_upload task and deliver its completion"]
pub struct FrameHandover {
    /// Whether the completion replaced the shown image.
    pub accepted: bool,
    /// Allocation for the newest pending frame; run it and deliver its result.
    pub next_upload: Option<Task<ReadyFrame>>,
}

impl LatestFramePresenter {
    /// Start with no shown, uploading or pending frame.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start empty and allocate in `window`'s renderer, the one that draws the
    /// frames; see [`FramePresenter::for_window`].
    #[must_use]
    pub fn for_window(window: iced::window::Id) -> Self {
        Self {
            presenter: FramePresenter::for_window(window),
            ..Self::default()
        }
    }

    /// Start one allocation or replace the pending pixel buffer.
    /// Run any returned task and deliver its completion to [`Self::ready`].
    pub fn offer(&mut self, frame: Rgba8Frame) -> Option<Task<ReadyFrame>> {
        if self.in_flight.is_some() {
            self.pending = Some(frame);
            None
        } else {
            Some(self.start(frame))
        }
    }

    fn start(&mut self, frame: Rgba8Frame) -> Task<ReadyFrame> {
        let task = self.presenter.offer(frame);
        self.in_flight = Some(self.presenter.latest);
        task
    }

    /// Accept a matching completion and admit the newest pending frame, even after clear.
    pub fn ready(&mut self, frame: ReadyFrame) -> FrameHandover {
        if self.in_flight != Some(frame.token) {
            return FrameHandover {
                accepted: false,
                next_upload: None,
            };
        }
        self.in_flight = None;
        let accepted = self.presenter.ready(frame);
        let next_upload = self.pending.take().map(|pending| self.start(pending));
        FrameHandover {
            accepted,
            next_upload,
        }
    }

    /// Clear shown/pending pixels and reject the active upload's presentation.
    /// Its completion must still be delivered to release admission and drain a new source.
    pub fn clear(&mut self) {
        self.pending = None;
        self.presenter.clear();
    }

    /// The shown image handle, retained while a newer upload is pending.
    #[must_use]
    pub fn handle(&self) -> Option<&iced::widget::image::Handle> {
        self.presenter.handle()
    }
    /// Dimensions of the shown frame in pixels.
    #[must_use]
    pub fn size(&self) -> Option<(u32, u32)> {
        self.presenter.size()
    }
    /// The shown RGBA pixels, suitable for saving the current frame.
    #[must_use]
    pub fn pixels(&self) -> Option<&[u8]> {
        self.presenter.pixels()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzui_core::media::{FrameSeq, Generation};
    use iced_test::futures::futures::{StreamExt, executor::block_on};
    use iced_test::runtime::{Action, image};

    fn frame(value: u8) -> Rgba8Frame {
        Rgba8Frame::new(
            Generation::FIRST,
            FrameSeq(u64::from(value)),
            2,
            2,
            vec![value; 16],
        )
        .unwrap()
    }

    struct Upload {
        handle: iced::widget::image::Handle,
        response: iced_test::futures::futures::channel::oneshot::Sender<
            Result<image::Allocation, image::Error>,
        >,
        stream: iced_test::futures::BoxStream<Action<ReadyFrame>>,
    }

    impl Upload {
        fn dispatch(task: Task<ReadyFrame>) -> Self {
            let (window, upload) = Self::dispatch_in(task);
            assert!(window.is_none(), "untargeted frame named a window");
            upload
        }

        fn dispatch_in(task: Task<ReadyFrame>) -> (Option<iced::window::Id>, Self) {
            let mut stream = iced_test::runtime::task::into_stream(task).unwrap();
            let (window, handle, response) = match block_on(stream.next()) {
                Some(Action::Image(image::Action::Allocate(handle, response))) => {
                    (None, handle, response)
                }
                Some(Action::Image(image::Action::AllocateIn(window, handle, response))) => {
                    (Some(window), handle, response)
                }
                _ => panic!("offered frame did not reach the image allocator"),
            };
            (
                window,
                Self {
                    handle,
                    response,
                    stream,
                },
            )
        }

        fn pixel(&self) -> u8 {
            match &self.handle {
                iced::widget::image::Handle::Rgba { pixels, .. } => pixels[0],
                _ => panic!("RGBA handle expected"),
            }
        }

        fn complete(self) -> ReadyFrame {
            let Self {
                response,
                mut stream,
                ..
            } = self;
            response.send(Err(image::Error::Unsupported)).unwrap();
            let Some(Action::Output(ready)) = block_on(stream.next()) else {
                panic!("allocation did not publish its completion")
            };
            ready
        }
    }

    #[test]
    fn clear_rejects_old_presentation_but_its_completion_drains_the_new_source() {
        let mut presenter = LatestFramePresenter::new();
        let old = Upload::dispatch(presenter.offer(frame(1)).unwrap());
        assert!(presenter.offer(frame(2)).is_none());
        presenter.clear();
        assert!(presenter.handle().is_none() && presenter.pending.is_none());
        assert!(
            presenter.offer(frame(3)).is_none(),
            "clear must not overlap the retired allocation"
        );
        assert!(presenter.offer(frame(4)).is_none());
        let old_ready = old.complete();
        let duplicate = old_ready.clone();
        let handover = presenter.ready(old_ready);
        assert!(!handover.accepted && presenter.handle().is_none());
        let new = Upload::dispatch(handover.next_upload.expect("new source allocation"));
        assert_eq!(new.pixel(), 4);
        assert!(presenter.offer(frame(5)).is_none());
        let stale = presenter.ready(duplicate);
        assert!(!stale.accepted && stale.next_upload.is_none());
        let handover = presenter.ready(new.complete());
        assert!(handover.accepted);
        assert_eq!(presenter.pixels().unwrap()[0], 4);
        let newest = Upload::dispatch(handover.next_upload.unwrap());
        assert_eq!(newest.pixel(), 5);
        let handover = presenter.ready(newest.complete());
        assert!(handover.accepted && handover.next_upload.is_none());
        assert_eq!(presenter.pixels().unwrap()[0], 5);
    }

    #[test]
    fn a_window_presenter_keeps_its_frame_until_the_next_lands_in_that_window() {
        let window = iced::window::Id::unique();
        let mut presenter = LatestFramePresenter::for_window(window);
        let (target, first) = Upload::dispatch_in(presenter.offer(frame(1)).unwrap());
        assert_eq!(target, Some(window));
        assert!(presenter.ready(first.complete()).accepted);
        let (target, second) = Upload::dispatch_in(presenter.offer(frame(2)).unwrap());
        assert_eq!(
            target,
            Some(window),
            "every frame goes to the drawing window"
        );
        assert!(presenter.offer(frame(3)).is_none());
        assert_eq!(
            presenter.pixels().unwrap()[0],
            1,
            "the shown frame stays until its successor's allocation completes"
        );
        let handover = presenter.ready(second.complete());
        assert_eq!(presenter.pixels().unwrap()[0], 2);
        let (target, pending) = Upload::dispatch_in(handover.next_upload.unwrap());
        assert_eq!((target, pending.pixel()), (Some(window), 3));
    }
}
