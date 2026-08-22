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
