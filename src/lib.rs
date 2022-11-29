// Rough plans.
// * API package contains a VM driver API that can abstract over WASM or fake VM.
// No dependencies on either the FVM internals or on built-in actors.
// Tests in the built-in actors (or other actor) development repos can be written in terms of this API.
// They can execute against a fake VM (from their own repo) or a real VM (here).
// * VM package contains an implementation of the driver API in terms of the real FVM.
// * A hookup package imports test scripts from actors and the VM package and wires them together.
//
// Two potential destinations for these packages:
// * Move the API into FVM shared package and the VM package into FVM testing.
// Import the API into builtin-actors from there.
// Note that this VM has a slightly different target than the existing testing/integration code,
// so would not fully replace that.
// Then all that's left here is the hookup package (and we could rename to builtin-actors-workbench).
// * Move the API into a new crate with ~no deps, import into built-in actors.
// Move the VM package into a crate with deps on FVM.
// Create a new repo for hookup (builtin-actors-workbench).


// This API could move to built-in actors repo, or FVM *shared* repo, or
// stay here to be depended on by a builtin-workbench.
// * VM package contains low-level VM wrapper, primitives, but some abstraction over FVM's raw API.
// This package could move into the FVM repo, perhaps replacing the current testing/integration package.
// An adapter package implements the API in terms of the VM.
// Tests in built-in actors are implemented in terms of the driver API, and can be executed
// against a fake VM there, or a real VM from builtin-workbench.


pub fn add(left: usize, right: usize) -> usize {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
