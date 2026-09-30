use crate::provider::TEST_PATH_OVERRIDE;
#[test]
fn parallel_provider_map_keeps_order_and_the_test_path_override() {
    crate::provider::set_test_provider_path("/fake/provider/bin");
    let items: Vec<usize> = (0..20).collect();
    let results = crate::provider::parallel_provider_map(&items, 4, |item| {
        let path = TEST_PATH_OVERRIDE.with(|p| p.borrow().clone());
        (*item, path)
    });
    crate::provider::clear_test_provider_path();
    assert_eq!(
        results.iter().map(|(item, _)| *item).collect::<Vec<_>>(),
        items
    );
    assert!(results
        .iter()
        .all(|(_, path)| path.as_deref() == Some("/fake/provider/bin")));
}
