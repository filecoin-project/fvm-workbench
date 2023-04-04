# FVM Workbench
This repo provides a stand-alone Filecoin Virtual Machine, i.e. one that runs without needing
a full Filecoin node like Lotus.
This supports running native (i.e. WASM) actors in a controlled environment,
and includes tools for analysis of execution traces.
A conventional log message format supports the notion of trace spans, 
allowing fine-grained analysis of gas consumption.

The repo also provides an abstraction over the VM implementation that can be implemented by
a proxy or light-weight fake VM.
This allows test scripts to be written that do not depend on the FVM directly,
independent even of compiling the actors to WASM.
Such tests can be executed quickly and with first-class debugging support with a fake VM that
uses the high level language's native debugging tools (e.g. Rust).
Then exactly the same test can be executed on a real VM for gas analysis.

## Crates
The repo is divided into several crates in order to limit dependencies.

### `api`
The `api` crate provides an API for setting up a VM and executing messages.
The crate does not depend on an FVM implementation, 
only the shared libraries commonly used by actors.
This crate can thus be imported directly into actor repositories, 
and integration tests written there without introducing a dependency on the full FVM.

### `vm`
The `vm` crate implements the API in terms of a real FVM,
imported from the reference implementation.
It provides methods to initialise the VM and install actors that make 
as few assumptions as possible about how you want to use it.
This crate does not depend on the built-in actors implementation, 
but users will need to install built-in actors for the VM to function.

*For Apple-silicon Macs* you will need the following in `.cargo/config.toml` in order to compile
the proof crates.

```
[build]
target = "x86_64-apple-darwin"
```

### `builtin`
The `builtin` crate depends on the built-in actors implementation 
and provides methods for establishing initial state in a VM, which depends on the actors in use.
The `builtin/tests/hookup.rs` "test" demonstrates initialisation and use with the `vm` crate.

This crate is intended to also directly execute the integration tests
imported from the built-in actors repo, once those tests are adapted to the API provided above.

## License
This repository is dual-licensed under Apache 2.0 and MIT terms.

Copyright 2022-2023. Protocol Labs, Inc.