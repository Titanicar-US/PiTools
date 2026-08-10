use std::sync::atomic::{AtomicU64, Ordering};

/// Low-cardinality process counters exposed in Prometheus text format.
#[derive(Debug, Default)]
pub struct Metrics {
    webhook_received: AtomicU64,
    webhook_rejected: AtomicU64,
    webhook_accepted: AtomicU64,
    webhook_duplicate: AtomicU64,
    webhook_conflict: AtomicU64,
}

impl Metrics {
    pub fn record_webhook_received(&self) {
        self.webhook_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_webhook_rejected(&self) {
        self.webhook_rejected.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_webhook_accepted(&self) {
        self.webhook_accepted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_webhook_duplicate(&self) {
        self.webhook_duplicate.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_webhook_conflict(&self) {
        self.webhook_conflict.fetch_add(1, Ordering::Relaxed);
    }

    pub fn render_prometheus(&self) -> String {
        let mut output = String::new();
        for (name, help, value) in [
            (
                "pitools_webhook_received_total",
                "Total webhook requests received.",
                self.webhook_received.load(Ordering::Relaxed),
            ),
            (
                "pitools_webhook_rejected_total",
                "Total webhook requests rejected before enqueue.",
                self.webhook_rejected.load(Ordering::Relaxed),
            ),
            (
                "pitools_webhook_accepted_total",
                "Total webhook deliveries accepted.",
                self.webhook_accepted.load(Ordering::Relaxed),
            ),
            (
                "pitools_webhook_duplicate_total",
                "Total duplicate webhook deliveries observed.",
                self.webhook_duplicate.load(Ordering::Relaxed),
            ),
            (
                "pitools_webhook_conflict_total",
                "Total delivery identifier conflicts observed.",
                self.webhook_conflict.load(Ordering::Relaxed),
            ),
        ] {
            output.push_str(&format!(
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"
            ));
        }
        output
    }
}
