use fil_actors_integration_tests::tests::*;
use fil_actors_integration_tests::TEST_REGISTRY;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_builtin_actors::setup;

#[test]
fn withdraw_balance_success() {
    let w = setup();
    withdraw_balance_success_test(&w);
}

// simple test that does invariants checking at the end
#[test]
fn change_owner_success() {
    let w = setup();
    change_owner_success_test(&w);
}

#[test]
fn account_authenticate_message() {
    let w = setup();
    account_authenticate_message_test(&w);
}

#[test]
fn test_registry() {
    let registry = TEST_REGISTRY.lock().unwrap();
    for (name, (speed, test)) in registry.iter() {
        println!("{}: ({})", name, speed);
        if speed < &1 {
            let w = setup();
            test(&w);
            let maybe_trace = w.peek_execution_trace();
            if let Some(trace) = maybe_trace {
                println!("{}", trace.format());
                let analysis = TraceAnalysis::build(trace.clone());
                println!("{}", analysis.format_spans());
            }
            println!("===============================================\n");
        } else {
            println!("skipping");
            println!("===============================================\n");
        }
    }
}
