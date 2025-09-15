#[cfg(test)]
mod tests {
    use node_lib::metrics::*;

    #[test]
    fn metrics_counter_basic_increment() {
        // If metrics exposes a counter type, exercise a simple increment path.
        // Many repos use lazy_static or simple helpers; adapt if the symbol differs.
        let mut m = Metrics::default();
        m.increment("test_counter");
        assert!(m.get("test_counter") >= 1);
    }
}
