# rahti-macros

Procedural macros for the [Rahti](https://crates.io/crates/rahti) web
framework: `html!`, `#[component]`, `#[rpc]`, and `#[socket]`.

Not used directly — every macro is re-exported by the `rahti` crate, which is
the dependency an application declares. This crate exists as a separate
package only because proc-macros must live in their own crate.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option.
