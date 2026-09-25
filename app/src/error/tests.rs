use super::*;

use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Once};

use axum::http::{StatusCode, header};

static TRACING_TEST_ACTIVE: AtomicBool = AtomicBool::new(false);
static TRACING_TEST_INITIALISED: Once = Once::new();
static TRACING_TEST_OUTPUT: Mutex<Option<Arc<Mutex<Vec<u8>>>>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct TracingTestWriter;

impl Write for TracingTestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let output = TRACING_TEST_OUTPUT.lock().unwrap().clone();
        if let Some(output) = output
            && let Ok(mut bytes_out) = output.lock()
        {
            bytes_out.extend_from_slice(bytes);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct TracingTestGuard {
    output: Arc<Mutex<Vec<u8>>>,
}

impl TracingTestGuard {
    fn output(&self) -> String {
        let bytes = self
            .output
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        String::from_utf8(bytes).unwrap_or_default()
    }
}

impl Drop for TracingTestGuard {
    fn drop(&mut self) {
        if let Ok(mut output) = TRACING_TEST_OUTPUT.lock() {
            *output = None;
        }
        TRACING_TEST_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    }
}

fn tracing_test_guard() -> TracingTestGuard {
    while TRACING_TEST_ACTIVE
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::Acquire,
            std::sync::atomic::Ordering::Relaxed,
        )
        .is_err()
    {
        std::thread::yield_now();
    }
    TRACING_TEST_INITIALISED.call_once(|| {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .without_time()
            .with_ansi(false)
            .with_writer(|| TracingTestWriter)
            .try_init()
            .expect("test tracing subscriber should initialise once");
    });
    let output = Arc::new(Mutex::new(Vec::new()));
    *TRACING_TEST_OUTPUT.lock().unwrap() = Some(output.clone());
    TracingTestGuard { output }
}

#[derive(Debug)]
struct SecretSource;

impl fmt::Display for SecretSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("api_key=do-not-log")
    }
}

impl Error for SecretSource {}

#[test]
fn debug_logs_and_browser_response_do_not_expose_the_source_message() {
    let tracing = tracing_test_guard();
    let error = AppError::new("call provider", SecretSource);
    let debug = format!("{error:?}");
    assert!(debug.contains("call provider"));
    assert!(debug.contains("SecretSource"));
    assert!(!debug.contains("do-not-log"));

    let response = error.into_response();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    let output = tracing.output();
    assert!(output.contains("operation=\"call provider\""), "{output}");
    assert!(
        output.contains("source=\"frinkworks::error::tests::SecretSource\""),
        "{output}"
    );
    assert!(!output.contains("do-not-log"), "{output}");
}
