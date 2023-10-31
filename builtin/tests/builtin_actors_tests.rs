use fil_actors_integration_tests::tests::change_owner_success_test;
use fil_actors_integration_tests::tests::withdraw_balance_success_test;
use fil_actors_runtime::test_utils::FakePrimitives;
use fvm_workbench_builtin_actors::setup;

#[test]
fn withdraw_balance_success() {
    let w = setup(Box::<FakePrimitives>::default());
    withdraw_balance_success_test(&w);
}

// simple test that does invariants checking at the end
#[test]
fn change_owner_success() {
    let w = setup(Box::<FakePrimitives>::default());
    change_owner_success_test(&w);
}
