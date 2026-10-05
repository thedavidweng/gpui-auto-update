//! Smoke test that the GPUI test harness builds and runs on every CI
//! platform, so later facade tests start from a known-good toolchain.

use gpui::{AppContext as _, TestAppContext};

struct Counter(u32);

#[gpui::test]
fn entities_update_and_notify_observers(cx: &mut TestAppContext) {
    let counter = cx.new(|_| Counter(0));
    let notified = cx.new(|_| 0u32);
    let _subscription = cx.update(|cx| {
        let notified = notified.clone();
        cx.observe(&counter, move |_, cx| notified.update(cx, |n, _| *n += 1))
    });

    counter.update(cx, |c, cx| {
        c.0 += 1;
        cx.notify();
    });
    cx.run_until_parked();

    assert_eq!(counter.read_with(cx, |c, _| c.0), 1);
    assert_eq!(notified.read_with(cx, |n, _| *n), 1);
}
