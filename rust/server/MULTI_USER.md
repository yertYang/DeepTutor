# Native accounts and permission boundaries

The Rust backend supports local accounts, administrator management, revocable
cookie/Bearer sessions, model and tool grants, and separate storage for ordinary
accounts. The API remains bound to loopback; publish the Next.js frontend for
network access.

## Enable accounts

In local mode, open **User Management** in the sidebar. Create an administrator
and choose **Enable multi-user mode**. This saves `enabled: true` in
`data/user/settings/auth.json`, signs in the administrator, and takes effect
immediately. If accounts already exist, setup requires an existing administrator's
password. The existing local workspace stays in the administrator scope.

Alternatively set `DEEPTUTOR_AUTH_ENABLED=true` before starting, or save:

```json
{
  "enabled": true,
  "allow_registration": false,
  "token_expire_hours": 24,
  "cookie_secure": false,
  "allowed_origins": []
}
```

Restart after editing the file directly. An explicit environment value overrides
the file on startup. With an empty account store, `/register` creates the first
administrator even if public registration is disabled. Later public registration
requires `allow_registration: true` (the compatibility default). Registered users
cannot choose an administrator role. Administrators can always create ordinary
accounts and import `username,password,preset` CSV files (maximum 500 rows/2 MiB).

Use `cookie_secure: true` behind HTTPS. Same-origin frontend requests work through
the Next.js proxy. `allowed_origins` is for explicit direct cross-origin API
clients; arbitrary credentialed origins are rejected.

## Account management

`/admin/users` supports creation, import, deletion, batch deletion, role changes,
learner profiles, and model/tool assignments. Administrator-assigned passwords
must be changed in **My profile** before using workspace APIs. Password changes
rotate the current session and revoke older sessions. Logout, deletion and role
changes revoke affected sessions; existing chat and voice sockets also recheck
their sessions. Users cannot delete themselves or change their own role, and the
last administrator cannot be removed.

New ordinary accounts start with no administrator-owned models. Open **Manage
assignments**, select specific models and save. Provider credentials stay on the
server. Guessing a profile/model id does not bypass a grant. Tool allowlists are
checked both when offering tools and when executing a tool call.

Users may also open **My profile → Personal models and API keys** and configure
their own provider URL, model identifier and Token/API key. Personal and assigned
models coexist in the chat selector. Personal credentials are stored only in
that account's settings, returned masked, and preserved when a saved masked value
is submitted unchanged. Assigned connections stay read-only. Revoking an assigned
model leaves personal connections intact.

Ordinary-account runtime profile ids use separate `personal:` and `assigned:`
namespaces, so matching personal/administrator ids cannot collide or restore the
wrong credential. Provider references and secrets always resolve against the
actual owner's catalog, including on each resumed upstream model request.

## Storage and compatibility

- Deployment identities and hashed session tokens: `data/system/auth/native.sqlite3`.
- Administrator workspace: `data/user` (compatible with the previous admin scope).
- Ordinary account workspace: `data/users/<stable-account-id>/user`.
- Model/tool grants: `data/system/grants/<stable-account-id>.json`.
- Personal provider keys: `data/users/<stable-account-id>/user/settings/model_catalog.json`.

Legacy `data/system/auth/users.json`, `data/user/auth_users.json`, and legacy
`multi-user/_system/auth/users.json` accounts are imported once. Bcrypt password
hashes, stable ids, roles and profile metadata are preserved. The old source files
are not overwritten. Invalid identity stores fail closed. PocketBase-backed
identities still require a separate migration; they are not silently replaced.

All workspace HTTP routes and WebSockets receive an application selected from
the verified session. Client user identifiers, workspace names, document ids,
turn ids and submission ids cannot switch account databases. Ordinary users
cannot change deployment provider settings, use administrator browser-testing services, or
access deployment metrics. Administrators share the deployment's admin workspace;
ordinary users' notes, sessions, knowledge sources, memory and settings live in
their own scope.

Recovery packages include `data/user`, `data/users`, `data/system`, and registered
administrator workspaces. Restoring rebases ordinary-account workspace paths and
invalidates copied login sessions. Deleting an account revokes access and retains
its old data directory; a recreated username gets a new id and cannot inherit it.

Administrators can authorize guardian relationships for learner accounts in
**Manage assignments**. Each relationship independently grants material
assignment, restriction changes, summary reports and credential resets.
Guardians use **My profile → Guardian relationships**. Learners can revoke their
own relationships there. Every operation rechecks the relationship and its
permission. Resets revoke sessions and require a first-login password change.

Material assignment copies immutable native reading snapshots into the learner's
account. Removing an assignment hides it from the reading list and rejects direct
item requests. The retained snapshot, progress and annotations allow later
reassignment without losing the learner's work. Restriction changes preserve
model grants and material assignments.

Legacy book editing, partner/skill sharing and MCP/host
execution grants have not migrated to the native runtime. Their controls are
hidden in native account management; unsupported grants are rejected. Learner
accounts retain a restrictive chat/reading policy and cannot use host tools.

## Verification

```sh
cargo test --locked -p deeptutor-server --test multi_user
```

The integration suite uses disposable homes and real HTTP/WebSocket servers to
check account bootstrap, concurrent registration, legacy import, temporary
passwords, admin-only routes, cross-account requests, model grants, logout,
revocation, bulk operations, cross-origin rejection, and backup/restore.

The browser acceptance runner expects a separate, disposable frontend/backend:

```sh
AUTH_QA_WEB_URL=http://127.0.0.1:3799 AUTH_QA_ALLOW_SETUP=1 \
  node scripts/native-auth-acceptance.mjs
```

It creates synthetic accounts, exercises the administrator form, assigns a model,
checks mobile access and first-login password changes, and captures screenshots
under `dist/auth-acceptance`.
