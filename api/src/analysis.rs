use crate::trace::{ExecutionEvent, ExecutionTrace};
use std::collections::HashMap;
use std::fmt::Display;
use std::ops::{Add, AddAssign};
use num_format::{Locale, ToFormattedString};
use itertools::Itertools;

/// Analysis of an execution trace.
/// Analysis takes the form of a collection of spans, each summarising some section of
/// the trace.
/// Spans may overlap. Some spans may be "children" of others, in which case the "total"
/// gas amounts of the parent will include the gas amounts of the children.
pub struct TraceAnalysis {
    spans: Vec<Span>,
}

impl TraceAnalysis {
    /// Builds a new analysis from an execution trace.
    /// Spans are inferred from (1) Calls (inter-actor message sends), and
    /// (2) specially formatted log messages.
    /// This inference from log messages might be better replaced by a primitive supported
    /// by the FVM directly, in the future.
    pub fn build(trace: ExecutionTrace) -> TraceAnalysis {
        // Collect all spans, in the order they are opened.
        let mut spans = vec![Span::new("Root".to_string())];
        // Call spans are nested, and this stack holds indices of the spans that are currently open.
        let mut call_stack = vec![0];
        // Non-call spans are independent, not nested.
        // Multiple open spans may account for the same gas charges.
        // Maps name to list of span indices with that label.
        let mut named_spans: HashMap<String, Vec<usize>> = HashMap::new();
        // Accumulate gas directly consumed by each span
        for event in trace.events() {
            match event {
                ExecutionEvent::GasCharge { name, compute_milli, storage_milli } => {
                    // Add gas to top span in the call stack.
                    let charge = GasCharge::new_millis(*compute_milli, *storage_milli);
                    let top_span = spans.get_mut(*call_stack.last().unwrap()).unwrap();
                    top_span.add_self_gas(name.to_string(), charge);
                    // Add gas to all open named spans.
                    for span in named_spans.values_mut().flatten() {
                        spans.get_mut(*span).unwrap().add_self_gas(name.to_string(), charge)
                    }
                }
                ExecutionEvent::Call { from, to, method, .. } => {
                    // Add a new span and push onto call stack.
                    let span_id = format!("{}-Call({}->{}::{})", spans.len(), from, to, method);
                    spans.push(Span::new(span_id));
                    call_stack.push(spans.len() - 1);
                }
                ExecutionEvent::CallReturn { .. }
                | ExecutionEvent::CallAbort { .. }
                | ExecutionEvent::CallError { .. } => {
                    // Pop from call stack
                    let closed_idx = call_stack.pop().unwrap();
                    let closed_span = spans.get_mut(closed_idx).unwrap();
                    closed_span.add_self_to_total_gas();

                    // Add closed span total gas to parent's total gas.
                    let top_idx = call_stack.last().unwrap();
                    let closed_span = spans.get(closed_idx).unwrap().clone();
                    spans.get_mut(*top_idx).unwrap().add_other_to_total_gas(&closed_span);
                }
                ExecutionEvent::Log { msg } => {
                    // Infer spans from log messages.
                    // This is pretty yukky, but a prototype until adding a first class span concept
                    // to the VM.
                    let start_pattern = "SpanStart:";
                    let end_pattern = "SpanEnd:";
                    if let Some(idx) = msg.find(start_pattern) {
                        let label = msg[idx + start_pattern.len()..].trim();
                        spans.push(Span::new(format!("{}-Span({})", spans.len(), label)));
                        named_spans.entry(label.to_string()).or_default().push(spans.len() - 1);
                    } else if let Some(idx) = msg.find(end_pattern) {
                        let label = msg[idx + end_pattern.len()..].trim();
                        // FIXME: this unwrap if the closed label isn't found is fragile,
                        // should be done differently.
                        let closed_idx = named_spans.get_mut(label).unwrap().pop().unwrap();
                        let closed_span = spans.get_mut(closed_idx).unwrap();
                        closed_span.add_self_to_total_gas();

                    }
                }
            }
        }
        // Close the root span.
        let top_idx = call_stack.pop().unwrap();
        assert!(call_stack.is_empty());
        let root_span = spans.get_mut(top_idx).unwrap();
        root_span.add_self_to_total_gas();

        TraceAnalysis { spans }
    }

    pub fn format_spans(&self) -> String {
        self.spans.iter().map(|e| format!("{}", e)).join("\n")
    }
}

/// An instrumentation record covering a period of an execution trace.
// TODO: generalise the span model to represent nesting, parents.
// Defer calculations of total gas to query time, traversing children.
#[derive(Clone, Debug)]
pub struct Span {
    id: String,
    self_gas: HashMap<String, GasCharge>,
    self_gas_sum: GasCharge,
    total_gas: HashMap<String, GasCharge>,
    total_gas_sum: GasCharge,
}

impl Span {
    pub fn new(id: String) -> Self {
        Self {
            id,
            self_gas_sum: GasCharge::zero(),
            self_gas: HashMap::new(),
            total_gas_sum: GasCharge::zero(),
            total_gas: HashMap::new(),
        }
    }

    /// Adds to this span's self gas.
    fn add_self_gas(&mut self, label: String, c: GasCharge) {
        self.self_gas_sum += c;
        *self.self_gas.entry(label).or_insert_with(GasCharge::zero) += c;
    }

    /// Adds this span's self gas to its total gas.
    fn add_self_to_total_gas(&mut self) {
        self.total_gas_sum += self.self_gas_sum;
        for (label, c) in self.self_gas.iter() {
            *self.total_gas.entry(label.to_string()).or_insert_with(GasCharge::zero) += *c;
        }
    }

    /// Adds total gas from another span to the totals for this one.
    fn add_other_to_total_gas(&mut self, other: &Span) {
        self.total_gas_sum += other.total_gas_sum;
        for (label, c) in &other.total_gas {
            *self.total_gas.entry(label.clone()).or_insert_with(GasCharge::zero) += *c;
        }
    }
}

impl Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut self_gas_bits = vec![("sum", self.self_gas_sum.total())];
        for (k, v) in &self.self_gas {
            self_gas_bits.push((k.as_str(), v.total()));
        }

        write!(
            f,
            "Span[{}, self: {{{}}}, total: {{{}}}]",
            self.id,
            format_gas_bits(self.self_gas_sum, &self.self_gas),
            format_gas_bits(self.total_gas_sum, &self.total_gas),
        )
    }
}

fn format_gas_bits(sum: GasCharge, charges: &HashMap<String, GasCharge>) -> String {
    let mut bits = vec![("sum", sum.total())];
    for (k, v) in charges {
        bits.push((k.as_str(), v.total()));
    }
    bits.sort_by(|a, b| b.1.cmp(&a.1)); // Sort by descending value, which will leave "sum" first.
    bits.iter().map(|(k, v)| format!("{}={}", k, v.to_formatted_string(&Locale::en))).join(", ")
}

/// A gas charge amount.
/// Gas is charged along multiple dimensions, though for now (FVM v2) it's accurate to simply sum
/// these dimensions into a total scalar gas cost.
#[derive(Copy, Clone, Debug)]
struct GasCharge {
    compute_milli: i64,
    storage_milli: i64,
}

impl GasCharge {
    /// Creates a new gas charge amount.
    /// Note that parameters are milligas.
    pub fn new_millis(compute_milli: i64, storage_milli: i64) -> Self {
        Self { compute_milli, storage_milli }
    }

    /// Creates a zero gas charge amount.
    pub fn zero() -> Self {
        Self::new_millis(0, 0)
    }

    /// Returns the total gas charge amount, summing the dimensions.
    pub fn total(&self) -> i64 {
        let millis = self.total_milli();
        let units = millis / 1000;
        // Round up
        if units != 0 && millis % 1000 != 0 {
            units + 1
        } else {
            units
        }
    }

    /// Returns the total gas charge amount in milligas.
    pub fn total_milli(&self) -> i64 {
        self.compute_milli + self.storage_milli
    }
}

impl Add for GasCharge {
    type Output = GasCharge;

    fn add(self, rhs: Self) -> Self::Output {
        GasCharge::new_millis(
            self.compute_milli + rhs.compute_milli,
            self.storage_milli + rhs.storage_milli,
        )
    }
}

impl AddAssign for GasCharge {
    fn add_assign(&mut self, rhs: Self) {
        self.compute_milli += rhs.compute_milli;
        self.storage_milli += rhs.storage_milli;
    }
}
