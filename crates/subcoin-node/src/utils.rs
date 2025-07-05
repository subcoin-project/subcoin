use indicatif::{ProgressBar, ProgressStyle};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A future that will always `yield` on the first call of `poll` but schedules the
/// current task for re-execution.
///
/// This is done by getting the waker and calling `wake_by_ref` followed by returning
/// `Pending`. The next time the `poll` is called, it will return `Ready`.
pub(crate) struct Yield(bool);

impl Yield {
    pub(crate) fn new() -> Self {
        Self(false)
    }
}

impl futures::Future for Yield {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut futures::task::Context<'_>,
    ) -> futures::task::Poll<()> {
        use futures::task::Poll;

        if !self.0 {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

pub(crate) fn show_progress_in_background(processed: Arc<AtomicUsize>, total: u64, msg: String) {
    std::thread::spawn(move || show_progress(processed, total, msg));
}

fn show_progress(processed: Arc<AtomicUsize>, total: u64, msg: String) {
    let pb = ProgressBar::new(total);

    pb.set_message(msg);

    pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg} [{bar:40}] {percent:.2}% ({pos}/{len}, {eta})")
            .unwrap()
            .progress_chars("##-"),
    );

    loop {
        let new = processed.load(Ordering::Relaxed) as u64;

        if new == total {
            pb.finish_and_clear();
            return;
        }

        pb.set_position(new);

        std::thread::sleep(Duration::from_millis(200));
    }
}
