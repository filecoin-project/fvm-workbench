use fil_actors_integration_tests::tests::withdraw_balance_success_test;
use fvm_workbench_builtin_actors::setup;

#[test]
fn withdraw_balance_test() {
    let w = setup();
    withdraw_balance_success_test(&w);
}
