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
