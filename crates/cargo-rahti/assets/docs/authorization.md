# Authorization

Authentication answers **who is calling**. Authorization answers **whether
that caller may perform this action on this resource**. Rahti keeps those
decisions separate:

| Concern | Mechanism |
| --- | --- |
| Require a signed session for a page | `private_routes` in `AuthSettings` |
| Require a signed session for an RPC | `#[rpc(auth)]` |
| Decide which action a session may perform | a named `rahti::authorization` ability |
| Decide which resource it may affect | a typed resource policy |

Rahti does not prescribe roles or a permissions table. An application may
store roles in its own session type and inspect them in a policy, but the
framework never infers what a role means.

## Define and install the gate

Keep the policy application-owned, normally beside the authentication policy
in `src/auth.rs`:

```rust
use rahti::authorization::Gate;

pub fn authorization() -> Gate {
    let mut gate = Gate::new();

    gate.define::<SessionUser, _>("reports.view", |user| {
        user.can_view_reports
    });

    gate.define_resource::<SessionUser, PostPolicy, _>(
        "posts.update",
        |user, post| user.id == post.owner_id,
    );

    gate
}
```

`SessionUser` must deserialize from the value passed to `auth::sign_in`.
`PostPolicy` can be a full model, but a small serializable policy input is
usually clearer:

```rust
#[derive(serde::Serialize, serde::Deserialize)]
struct PostPolicy {
    owner_id: i32,
}
```

Install the complete gate once during application startup, after auth and
before building the router:

```rust
rahti::auth::configure(auth::settings());
rahti::authorization::configure(auth::authorization());
```

A second `configure` replaces the gate. This makes router integration tests
deterministic; production startup should install it once.

Ability names are application-wide. Use stable, namespaced names such as
`posts.update` or `billing.refund`. Empty names, whitespace, NUL bytes, and
duplicate definitions are startup errors. An ability used by a handler but
not defined by the gate fails closed.

## Enforce inside the handler

Authentication and authorization are both required for a private mutation:

```rust
#[rpc(auth)]
pub async fn update_post(id: i32, title: String) -> rahti::Result<Post> {
    let post = Post::find_by_id(id)
        .await?
        .ok_or_else(|| {
            rahti::Error::new(axum::http::StatusCode::NOT_FOUND, "post not found")
        })?;

    rahti::authorization::authorize_resource(
        "posts.update",
        &PostPolicy { owner_id: post.owner_id },
    )?;

    // Perform the write only after authorization succeeds.
    update(post, title).await
}
```

`#[rpc(auth)]` answers 401 before parsing the payload when no valid session
exists. `authorize_resource` answers 403 when an authenticated session is not
allowed. The route guard only protects page loading; it does not protect an
RPC, API endpoint, or operation on a particular row.

Take caller identity from the signed session. Never trust a `user_id` argument
as proof of who is calling. If an identifier alone contains enough policy
information, authorize before querying the resource. Otherwise load only what
the policy needs, authorize, and then perform the side effect.

Policies are synchronous and should be deterministic over their session and
resource inputs. Database or network work belongs in the async handler before
the policy check.

## Gate forms

| Definition | Policy input |
| --- | --- |
| `define::<User>` | typed session only |
| `define_resource::<User, Resource>` | typed session and typed resource |
| `define_value` | raw JSON session and optional raw JSON resource |
| `before::<User>` | typed session and ability name, before every defined policy |

A `before` callback returns `Some(true)` to grant, `Some(false)` to deny, or
`None` to continue to the named rule. It is useful for a deliberate
administrator override:

```rust
gate.before::<SessionUser, _>(|user, _ability| {
    user.is_admin.then_some(true)
});
```

Undefined abilities never reach `before`; a misspelling cannot become an
administrator bypass.

## Inspecting without an error

Handlers normally use `authorize` or `authorize_resource`. Rendering code may
use `allows` or `allows_resource` to avoid showing an action the current
session cannot perform. This is presentation only: a hidden button is not a
security boundary, so the receiving handler must repeat the authorization
check.

For diagnostics or branching, `inspect` and `inspect_resource` return:

| `Decision` | Meaning |
| --- | --- |
| `Allowed` | the policy granted the ability |
| `Unauthenticated` | no valid request session exists |
| `Denied` | the policy rejected the session/resource |
| `Undefined` | no policy defines the ability |

Missing resources, session/resource type mismatches, malformed ability names,
and undefined abilities never grant access. Policy shape failures and enforced
undefined abilities are written to `.rahti/dev.log` in development without
exposing their details in the HTTP response.

## Testing

Test all three relevant paths through the real router:

1. no session receives 401 from `#[rpc(auth)]`;
2. a signed-in but unauthorized session receives 403;
3. an allowed session reaches the operation.

Also test ownership boundaries with two distinct users. A successful
self-update alone does not prove that changing the submitted resource id is
safe.
