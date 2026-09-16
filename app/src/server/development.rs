use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use axum::{Router, http::header, routing::get};
use rand::{rand_core::TryRng, rngs::SysRng};
use tower_livereload::LiveReloadLayer;

pub(super) fn with_live_reload(app: Router, static_dir: PathBuf) -> Router {
    let mut boot = [0u8; 16];
    SysRng
        .try_fill_bytes(&mut boot)
        .expect("system random number generator failed");
    let boot = u128::from_le_bytes(boot);
    let generation = Arc::new(AtomicU64::new(0));
    let revision = generation.clone();
    let live_reload = LiveReloadLayer::new().request_predicate(suppress_injection);
    let reloader = live_reload.reloader();
    let files = [
        static_dir.join("assets/main.js"),
        static_dir.join("assets/main.css"),
    ];
    let mut last = files.each_ref().map(|path| modified(path));
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let current = files.each_ref().map(|path| modified(path));
            if current != last {
                last = current;
                // Publish the revision before clients receive the reload event.
                generation.fetch_add(1, Ordering::Relaxed);
                reloader.reload();
            }
        }
    });

    app.route(
        "/_tower-livereload/revision",
        get(move || {
            let value = format!("{boot:032x}-{}", revision.load(Ordering::Relaxed));
            async move { ([(header::CACHE_CONTROL, "no-store")], value) }
        }),
    )
    .layer(live_reload)
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn suppress_injection<B>(_: &axum::http::Request<B>) -> bool {
    false
}
