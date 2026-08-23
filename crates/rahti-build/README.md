# rahti-build

The build step for the [Rahti](https://crates.io/crates/rahti) web
framework. A Rahti application calls it from `build.rs`:

```rust
fn main() {
    rahti_build::run();
}
```

On every relevant Cargo build it reads `rahti.config.json`, scans the route
convention files under `src/app/` and the components under
`src/components/`, validates route, RPC, and socket-name conflicts, and
generates the router, module declarations, and tooling manifest. It also
compiles or copies the stylesheet.

`cargo rahti new` writes all of this wiring for you — this crate is added to
`[build-dependencies]` by the scaffold, not by hand.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option.
