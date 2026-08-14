# rahti-ui

An example component library for the
[Rahti](https://crates.io/crates/rahti) web framework, written the way any
third-party library would be: it depends on `rahti` like a crate off
crates.io, marks its components with `#[component]`, and declares its
Tailwind sources and plain-CSS stylesheet in `[package.metadata.rahti]` so
an application's build picks its styles up automatically.

Install it with `cargo add rahti-ui`, then use its components as tags in
`html!`. Its main value is as the worked example of how to package a Rahti
component library of your own.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option.
