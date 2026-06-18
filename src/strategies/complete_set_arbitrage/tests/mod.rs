mod shell;

#[test]
fn strategy_tests_delegate_nt_order_contract_to_shared_execution() {
    let contract = crate::bolt_v3_order_execution::nt_order_management_contract();
    assert!(!contract.order_list_type.is_empty());
    assert!(!contract.submit_order_list_type.is_empty());
}
