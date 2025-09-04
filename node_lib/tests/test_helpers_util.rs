use node_lib::test_helpers::util::mk_hub_with_checks_flat;
use std::sync::Arc;

#[test]
fn mk_hub_with_checks_flat_rejects_bad_length() {
    // hub_fds length = 3 -> expected delays_flat length = 9
    let hub_fds = vec![1, 2, 3];
    let delays_flat = vec![0u64; 8]; // too short
    let checks: Vec<Arc<dyn node_lib::test_helpers::hub::HubCheck>> = Vec::new();
    let res = mk_hub_with_checks_flat(hub_fds, delays_flat, checks);
    assert!(res.is_err(), "expected error for bad delays_flat length");
}
