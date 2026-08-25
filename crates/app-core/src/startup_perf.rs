use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

pub fn start_run(component: &str) {
    if cfg!(test) {
        return;
    }
    let started = START.set(Instant::now()).is_ok();
    if started {
        append_line("run/start", component);
    } else {
        mark("run/already_started", component);
    }
}

pub fn mark(stage: impl AsRef<str>, detail: impl AsRef<str>) {
    if cfg!(test) {
        return;
    }
    let _ = START.get_or_init(Instant::now);
    append_line(stage.as_ref(), detail.as_ref());
}

pub fn measure<T, F>(stage: impl AsRef<str>, detail: impl AsRef<str>, f: F) -> T
where
    F: FnOnce() -> T,
{
    if cfg!(test) {
        return f();
    }
    let stage = stage.as_ref().to_string();
    let detail = detail.as_ref().to_string();
    mark(format!("{stage}/start"), &detail);
    let started = Instant::now();
    let result = f();
    mark(
        format!("{stage}/end"),
        format!("{} duration_ms={}", detail, started.elapsed().as_millis()),
    );
    result
}

fn append_line(stage: &str, detail: &str) {
    let elapsed_ms = START
        .get()
        .map(|start| start.elapsed().as_millis())
        .unwrap_or(0);
    tracing::info!(
        target: "startup_perf",
        stage = stage,
        detail = %detail.replace('\r', " ").replace('\n', " "),
        elapsed_ms,
        pid = std::process::id(),
        "startup mark"
    );
}
