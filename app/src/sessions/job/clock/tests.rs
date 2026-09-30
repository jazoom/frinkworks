use super::*;

#[test]
fn user_wait_time_does_not_enter_the_recorded_duration() {
    let mut clock = WorkClock::default();
    assert_eq!(clock.elapsed_ms(), None);
    clock.resume();
    clock.active_since = Some(Instant::now() - Duration::from_secs(3));
    clock.pause();
    let paused = clock.elapsed_ms().unwrap();
    assert!(paused >= 3_000);
    assert!(clock.active_since.is_none());
    clock.pause();
    assert_eq!(clock.elapsed_ms(), Some(paused));
    clock.resume();
    clock.active_since = Some(Instant::now() - Duration::from_secs(2));
    clock.pause();
    assert!(clock.elapsed_ms().unwrap() >= paused + 2_000);
}
