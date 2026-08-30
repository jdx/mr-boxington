use super::*;

#[test]
fn combined_budget_reserves_the_full_action_store_allowance() {
    let retention = RetentionSettings {
        target_max_bytes: Some(80),
        target_max_age: None,
        max_total_bytes: Some(100),
    };

    assert_eq!(target_budget(&retention, 70), Some(30));
    assert_eq!(target_budget(&retention, 120), Some(0));
}
