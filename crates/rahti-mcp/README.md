# rahti-mcp

`rahti-mcp` is Rahti's read-only Model Context Protocol server. It gives an
MCP client structured access to a project's generated manifest, public
configuration, convention documentation, database artifact inventory, and
bounded development diagnostics.

Run it over stdio from the Rahti project root:

```text
cargo run -p rahti-mcp -- --project .
```

The server never reads `.env` and does not reuse the application's browser
session. It reports whether RPCs and WebSockets require a session, while route
privacy remains a runtime application policy.

`list_rpcs` reads the generated `rpcContracts` inventory. It includes parameter
and success-response Rust types, required arguments, source locations, scoped
URLs, transports, and checking limits. A Rust type spelling is not a JSON
schema: unresolved representations require inspecting the declaration and
Serde behavior. Run `cargo check` to regenerate a missing contract inventory.

Projects created or upgraded with `--mcp` also enable Rahti's optional `mcp`
feature. Their generated application router exposes Streamable HTTP at
`/mcp/`, discovery at `/.well-known/mcp.json`, and public context at
`/llms.txt`. HTTP access uses `RAHTI_MCP_AUTH_TOKEN`; production hosts must
also be named by `RAHTI_MCP_ALLOWED_HOSTS`.
