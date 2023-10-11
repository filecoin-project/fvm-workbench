use fil_actors_integration_tests::tests::TEST_REGISTRY;
use fil_actors_integration_tests::tests::*;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_builtin_actors::setup;

#[test]
fn withdraw_balance_test() {
    let w = setup();
    withdraw_balance_success_test(&w);
}

#[test]
fn benchmark_builtin_actors() {
    for (test_name, test_fn) in TEST_REGISTRY.lock().unwrap().iter() {
        let w = setup();
        println!(
            "\n\n\n========================================================================\n\n\n"
        );
        println!("Running test: {}", test_name);
        test_fn(&w);
        let trace = w.peek_execution_trace().unwrap();
        println!("{}", trace.format());
        let analysis = TraceAnalysis::build(trace.clone());
        println!("{}", analysis.format_spans());
    }
}

// do not commit: run the problematic test here to isolate it's failure
#[test]
fn problematic_test() {
    let w = &setup();
    //    =============
    aggregate_bad_sector_number_test(w);
    // ============
}
