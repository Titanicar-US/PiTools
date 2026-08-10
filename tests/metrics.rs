use pitools::metrics::Metrics;

#[test]
fn prometheus_metrics_render_stable_webhook_counters() {
    let metrics = Metrics::default();
    metrics.record_webhook_received();
    metrics.record_webhook_accepted();
    metrics.record_webhook_duplicate();

    let rendered = metrics.render_prometheus();
    assert!(rendered.contains("pitools_webhook_received_total 1"));
    assert!(rendered.contains("pitools_webhook_accepted_total 1"));
    assert!(rendered.contains("pitools_webhook_duplicate_total 1"));
    assert!(rendered.ends_with('\n'));
}
