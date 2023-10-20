use fil_actors_integration_tests::tests::*;
use fil_actors_integration_tests::util::assert_invariants;
use fil_actors_integration_tests::TEST_REGISTRY;
use fil_actors_runtime::runtime::Policy;
use fvm_workbench_api::analysis::TraceAnalysis;
use fvm_workbench_builtin_actors::setup;

#[test]
fn fvm_workbench_is_compatible_with_builtins_invariants() {
    let w = &setup();
    assert_invariants(w, &Policy::default());
}

#[test]
fn benchmark_builtin_actors() {
    println!("TestRegistry: {} tests", TEST_REGISTRY.lock().unwrap().len());
    for (test_name, (speed, test_fn)) in TEST_REGISTRY.lock().unwrap().iter() {
        if *speed > 0 {
            println!("Skipping test: {} (speed {})", test_name, speed);
        } else {
            println!("Running test: {}", test_name);
            let w = setup();
            test_fn(&w);
            let trace = w.peek_execution_trace();
            if let Some(trace) = trace {
                println!("{}", trace.format());
                let analysis = TraceAnalysis::build(trace.clone());
                println!("{}", analysis.format_spans());
            }
        }
    }
}
