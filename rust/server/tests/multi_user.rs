use axum::{Router, routing::post};
use deeptutor_server::{Application, api, config::write_json};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

struct Server {
    _home: tempfile::TempDir,
    app: Arc<Application>,
    base: String,
    task: tokio::task::JoinHandle<()>,
    client: Client,
}

#[tokio::test]
async fn server_code_updates_require_admin_and_explicit_confirmation() {
    let server = Server::open(true).await;
    let admin = server.admin().await;
    server
        .json(
            Method::POST,
            "/api/auth/users",
            &admin,
            json!({"username":"updater-user","password":"updater-user-password","preset":"custom"}),
        )
        .await;
    let standard = server.activate("updater-user").await;
    for (method, path, body) in [
        (Method::GET, "/api/system/update", Value::Null),
        (Method::POST, "/api/system/update/check", json!({})),
        (Method::GET, "/api/system/update/job", Value::Null),
        (
            Method::PUT,
            "/api/system/update/settings",
            json!({"enabled":false}),
        ),
        (
            Method::POST,
            "/api/system/update",
            json!({"confirmation":"update-and-restart"}),
        ),
    ] {
        assert_eq!(
            server
                .request(method.clone(), path, "", body.clone())
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            server.request(method, path, &standard, body).await.status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        server
            .request(Method::POST, "/api/system/update", &admin, json!({}))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        server
            .request(
                Method::PUT,
                "/api/system/update/settings",
                &admin,
                json!({"enabled":"yes"})
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    server.close().await;
}
impl Server {
    async fn open(enabled: bool) -> Self {
        let home = tempfile::tempdir().unwrap();
        write_json(
            &home.path().join("data/user/settings/auth.json"),
            &json!({"enabled":enabled,"allow_registration":false}),
        )
        .unwrap();
        let app = Application::open(home.path()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = api::router(app.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            _home: home,
            app,
            base,
            task,
            client: Client::new(),
        }
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        cookie: &str,
        body: Value,
    ) -> reqwest::Response {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if !cookie.is_empty() {
            req = req.header("cookie", cookie);
        }
        if !body.is_null() {
            req = req.json(&body);
        }
        req.send().await.unwrap()
    }
    async fn json(&self, method: Method, path: &str, cookie: &str, body: Value) -> Value {
        let res = self.request(method, path, cookie, body).await;
        let status = res.status();
        let body = res.json::<Value>().await.unwrap();
        assert!(status.is_success(), "{path}: {status}: {body}");
        body
    }
    async fn cookie(&self, path: &str, body: Value) -> String {
        let res = self.request(Method::POST, path, "", body).await;
        assert_eq!(res.status(), StatusCode::OK);
        let header = res.headers().get("set-cookie").unwrap().to_str().unwrap();
        assert!(
            header.contains("HttpOnly")
                && header.contains("SameSite=Lax")
                && header.contains("Path=/")
        );
        header.split(';').next().unwrap().to_owned()
    }
    async fn admin(&self) -> String {
        self.json(
            Method::POST,
            "/api/auth/register",
            "",
            json!({"username":"admin","password":"admin-password"}),
        )
        .await;
        self.cookie(
            "/api/auth/login",
            json!({"username":"admin","password":"admin-password"}),
        )
        .await
    }
    async fn socket(
        &self,
        cookie: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = format!("{}/ws", self.base.replace("http:", "ws:"))
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("cookie", cookie.parse().unwrap());
        connect_async(request).await.unwrap().0
    }
    async fn activate(&self, name: &str) -> String {
        let temporary = self
            .cookie(
                "/api/auth/login",
                json!({"username":name,"password":format!("{name}-password")}),
            )
            .await;
        assert_eq!(
            self.request(Method::GET, "/api/sessions", &temporary, Value::Null)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        self.json(Method::GET, "/api/modules", &temporary, Value::Null)
            .await;
        assert_eq!(
            self.json(Method::GET, "/api/auth/profile", &temporary, Value::Null)
                .await["can_change_password"],
            true
        );
        let response=self.request(Method::PUT,"/api/auth/profile/password",&temporary,json!({"current_password":format!("{name}-password"),"new_password":format!("{name}-owned-password")})).await;
        assert_eq!(response.status(), StatusCode::OK);
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }
    async fn close(self) {
        self.app.shutdown().await;
        self.task.abort();
    }
}

#[tokio::test]
async fn authentication_management_scope_and_session_revocation() {
    let server = Server::open(true).await;
    assert_eq!(
        server
            .json(Method::GET, "/api/auth/status", "", Value::Null)
            .await["authenticated"],
        false
    );
    for path in [
        "/api/sessions",
        "/api/auth/users",
        "/api/native/status",
        "/api/settings",
    ] {
        assert_eq!(
            server
                .request(Method::GET, path, "", Value::Null)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            server
                .request(Method::GET, path, "dt_token=forged", Value::Null)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(
        connect_async(format!("{}/ws", server.base.replace("http:", "ws:")))
            .await
            .is_err()
    );
    let admin = server.admin().await;
    assert_eq!(
        server
            .json(Method::GET, "/api/native/status", &admin, Value::Null)
            .await["mode"],
        "multi-user"
    );
    assert_eq!(
        server
            .request(
                Method::POST,
                "/api/auth/register",
                "",
                json!({"username":"outsider","password":"long-password"})
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let alice=server.json(Method::POST,"/api/auth/users",&admin,json!({"username":"alice","password":"alice-password","preset":"standard","role":"admin"})).await;
    assert_eq!(alice["role"], "user");
    assert!(alice["hash"].is_null());
    assert_eq!(
        server
            .request(
                Method::POST,
                "/api/auth/users",
                &admin,
                json!({"username":"alice","password":"other-password"})
            )
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        server
            .request(
                Method::POST,
                "/api/auth/users",
                &admin,
                json!({"username":"short","password":"123"})
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    server
        .json(
            Method::POST,
            "/api/auth/users",
            &admin,
            json!({"username":"bob","password":"bob-password","preset":"custom"}),
        )
        .await;
    assert_eq!(
        server
            .request(
                Method::POST,
                "/api/auth/login",
                "",
                json!({"username":"alice","password":"wrong"})
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let alice_cookie = server.activate("alice").await;
    let bob_cookie = server.activate("bob").await;
    for path in [
        "/api/auth/users",
        "/api/multi-user/admin/resources",
        "/api/native/metrics",
        "/api/browser-testing",
    ] {
        assert_eq!(
            server
                .request(Method::GET, path, &alice_cookie, Value::Null)
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    for path in [
        "/api/settings/catalog",
        "/api/settings/ui/../apply",
        "/api/settings/providers/openai-codex/oauth/logout",
    ] {
        assert_eq!(
            server
                .request(Method::POST, path, &alice_cookie, json!({}))
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    assert_eq!(
        server
            .request(Method::DELETE, "/api/auth/users/admin", &admin, Value::Null)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        server
            .request(
                Method::PUT,
                "/api/auth/users/admin/role",
                &admin,
                json!({"role":"user"})
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let list = server
        .json(Method::GET, "/api/auth/users", &admin, Value::Null)
        .await;
    assert_eq!(list.as_array().unwrap().len(), 3);
    assert!(!list.to_string().contains("$2"));
    let hostile = server
        .client
        .post(format!("{}/api/auth/users", server.base))
        .header("cookie", &admin)
        .header("origin", "https://untrusted.example")
        .json(&json!({"username":"csrf-user","password":"csrf-password"}))
        .send()
        .await
        .unwrap();
    assert_eq!(hostile.status(), StatusCode::FORBIDDEN);
    let preflight = server
        .client
        .request(Method::OPTIONS, format!("{}/api/auth/users", server.base))
        .header("origin", "https://untrusted.example")
        .header(
            "access-control-request-headers",
            "x-deeptutor-frontend-host,content-type",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::FORBIDDEN);

    // Preferences and authored data are selected by the verified identity,
    // including when the caller supplies another user's id or workspace name.
    server
        .json(
            Method::PUT,
            "/api/settings/ui",
            &alice_cookie,
            json!({"language":"zh"}),
        )
        .await;
    server
        .json(
            Method::PUT,
            "/api/settings/ui",
            &bob_cookie,
            json!({"language":"en"}),
        )
        .await;
    assert_eq!(
        server
            .json(
                Method::GET,
                "/api/settings/ui?user_id=bob",
                &alice_cookie,
                Value::Null
            )
            .await["language"],
        "zh"
    );
    assert_eq!(
        server
            .json(Method::GET, "/api/settings/ui", &bob_cookie, Value::Null)
            .await["language"],
        "en"
    );
    let document = server
        .json(
            Method::POST,
            "/api/documents",
            &alice_cookie,
            json!({"title":"Alice private note","blocks":[]}),
        )
        .await;
    let id = document["id"].as_str().unwrap();
    let private = format!("/api/documents/{id}");
    assert_eq!(
        server
            .request(Method::GET, &private, &bob_cookie, Value::Null)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .request(Method::DELETE, &private, &bob_cookie, json!({}))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let others = server
        .json(
            Method::GET,
            &format!(
                "/api/documents?user_id={}&username=alice",
                alice["id"].as_str().unwrap()
            ),
            &bob_cookie,
            Value::Null,
        )
        .await;
    assert!(!others.to_string().contains("Alice private note"));
    assert!(
        !server
            .json(Method::GET, "/api/documents", &admin, Value::Null)
            .await
            .to_string()
            .contains("Alice private note")
    );

    // Neither a live socket nor a copied cookie survives logout or a role change.
    let mut socket = server.socket(&alice_cookie).await;
    socket
        .send(Message::Text(
            json!({"type":"ping","protocol_version":"2.0"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert!(
        socket
            .next()
            .await
            .unwrap()
            .unwrap()
            .to_text()
            .unwrap()
            .contains("pong")
    );
    server
        .json(Method::POST, "/api/auth/logout", &alice_cookie, json!({}))
        .await;
    assert_eq!(
        server
            .request(Method::GET, "/api/documents", &alice_cookie, Value::Null)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let closed = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
        .await
        .unwrap();
    assert!(
        closed.is_none()
            || closed
                .as_ref()
                .is_some_and(|v| v.is_err() || matches!(v, Ok(Message::Close(_))))
    );
    server
        .json(
            Method::PUT,
            "/api/auth/users/bob/role",
            &admin,
            json!({"role":"admin"}),
        )
        .await;
    assert_eq!(
        server
            .request(Method::GET, "/api/auth/users", &bob_cookie, Value::Null)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let bob_admin = server
        .cookie(
            "/api/auth/login",
            json!({"username":"bob","password":"bob-owned-password"}),
        )
        .await;
    server
        .json(Method::GET, "/api/auth/users", &bob_admin, Value::Null)
        .await;
    server
        .json(
            Method::PUT,
            "/api/auth/users/bob/role",
            &admin,
            json!({"role":"user"}),
        )
        .await;
    assert_eq!(
        server
            .request(Method::GET, "/api/auth/users", &bob_admin, Value::Null)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let alice_cookie = server
        .cookie(
            "/api/auth/login",
            json!({"username":"alice","password":"alice-owned-password"}),
        )
        .await;
    let changed = server
        .request(
            Method::PUT,
            "/api/auth/profile/password",
            &alice_cookie,
            json!({"current_password":"alice-owned-password","new_password":"changed-password"}),
        )
        .await;
    assert_eq!(changed.status(), StatusCode::OK);
    let new_cookie = changed.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert_eq!(
        server
            .request(Method::GET, "/api/auth/profile", &alice_cookie, Value::Null)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .json(Method::GET, "/api/auth/profile", new_cookie, Value::Null)
            .await["must_change_password"],
        false
    );
    server
        .json(Method::DELETE, "/api/auth/users/alice", &admin, Value::Null)
        .await;
    assert_eq!(
        server
            .request(Method::GET, "/api/auth/profile", new_cookie, Value::Null)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    server.close().await;
}

#[tokio::test]
async fn registration_race_bulk_operations_and_recovery_preserve_account_boundaries() {
    let server = Server::open(true).await;
    let responses = futures_util::future::join_all((0..3).map(|n| {
        server.request(
            Method::POST,
            "/api/auth/register",
            "",
            json!({"username":format!("race{n}"),"password":"race-password"}),
        )
    }))
    .await;
    assert_eq!(
        responses.iter().filter(|r| r.status().is_success()).count(),
        1
    );
    let winner = server.app.accounts.list().unwrap()[0]["username"]
        .as_str()
        .unwrap()
        .to_owned();
    let admin = server
        .cookie(
            "/api/auth/login",
            json!({"username":winner,"password":"race-password"}),
        )
        .await;
    let csv = "username,password,preset\nbackup-user,backup-user-password,standard\ndelete-user,delete-user-password,custom\nbackup-user,duplicate-password,standard\n";
    let upload = format!(
        "--auth-test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"users.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--auth-test--\r\n"
    );
    let imported = server
        .client
        .post(format!("{}/api/auth/users/import", server.base))
        .header("cookie", &admin)
        .header("content-type", "multipart/form-data; boundary=auth-test")
        .body(upload)
        .send()
        .await
        .unwrap();
    assert_eq!(imported.status(), StatusCode::OK);
    let result: Value = imported.json().await.unwrap();
    assert_eq!(result["created_count"], 2);
    assert_eq!(result["failed_count"], 1);
    let batch = server
        .json(
            Method::POST,
            "/api/auth/users/batch-delete",
            &admin,
            json!({"usernames":["delete-user",winner]}),
        )
        .await;
    assert_eq!(batch["deleted_count"], 1);
    assert_eq!(batch["failed_count"], 1);
    let cookie = server.activate("backup-user").await;
    let profile = server
        .json(Method::GET, "/api/auth/profile", &cookie, Value::Null)
        .await;
    let uid = profile["id"].as_str().unwrap().to_owned();
    let doc = server
        .json(
            Method::POST,
            "/api/documents",
            &cookie,
            json!({"title":"Private recovery note","content":"Only this account owns this note."}),
        )
        .await;
    let workspace = server
        .json(
            Method::POST,
            "/api/settings/workspace/registrations",
            &cookie,
            json!({"name":"Private workspace"}),
        )
        .await;
    let workspace_id = workspace["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let doc_id = doc["id"].as_str().unwrap().to_owned();
    let Server {
        _home: home,
        app,
        task,
        ..
    } = server;
    app.shutdown().await;
    task.abort();
    let _ = task.await;
    drop(app);
    let destination = tempfile::tempdir().unwrap();
    let package = destination.path().join("backup");
    deeptutor_server::recovery::create(home.path(), &package).unwrap();
    assert_eq!(
        deeptutor_server::recovery::verify(&package).unwrap()["verified"],
        true
    );
    let restored = destination.path().join("restored");
    deeptutor_server::recovery::restore(&package, &restored).unwrap();
    let app = Application::open(&restored).unwrap();
    assert_eq!(app.accounts.list().unwrap().len(), 2);
    assert!(
        app.accounts
            .resolve(admin.strip_prefix("dt_token="))
            .unwrap()
            .is_none()
    );
    assert!(
        app.accounts
            .login("backup-user", "backup-user-owned-password", false)
            .is_ok()
    );
    let user_root = restored.join("data/users").join(&uid).join("user");
    let store = deeptutor_server::store::Store::open(&user_root.join("chat_history.db")).unwrap();
    assert_eq!(
        deeptutor_server::documents::read(&store.connect().unwrap(), "", &doc_id).unwrap()["title"],
        "Private recovery note"
    );
    assert!(deeptutor_server::documents::read(&app.store.connect().unwrap(), "", &doc_id).is_err());
    let conn = rusqlite::Connection::open(user_root.join("runtime/workspaces.sqlite3")).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT payload FROM workspaces WHERE id=?",
            [&workspace_id],
            |r| r.get(0),
        )
        .unwrap();
    let workspace: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        workspace["path"],
        json!(
            user_root
                .canonicalize()
                .unwrap()
                .join("workspaces")
                .join(&workspace_id)
        )
    );
    app.shutdown().await;
}

#[tokio::test]
async fn local_setup_and_legacy_identity_survive_restart() {
    let server = Server::open(false).await;
    let local = server
        .json(Method::GET, "/api/auth/status", "", Value::Null)
        .await;
    assert_eq!(local["enabled"], false);
    assert_eq!(local["is_admin"], true);
    let admin = server
        .cookie(
            "/api/auth/setup",
            json!({"username":"owner","password":"owner-password"}),
        )
        .await;
    assert_eq!(
        server
            .request(Method::GET, "/api/sessions", "", Value::Null)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        server
            .json(Method::GET, "/api/auth/status", &admin, Value::Null)
            .await["username"],
        "owner"
    );
    assert_eq!(
        server
            .request(
                Method::POST,
                "/api/auth/setup",
                "",
                json!({"username":"attacker","password":"attacker-password"})
            )
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let root = server._home.path().to_owned();
    let user_id = server
        .json(Method::GET, "/api/auth/users", &admin, Value::Null)
        .await[0]["id"]
        .clone();
    server.app.shutdown().await;
    server.task.abort();
    drop(server.app);
    // Router task relinquishes the application and its home lock.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let reopened = Application::open(&root).unwrap();
    assert!(reopened.accounts.is_enabled());
    assert_eq!(reopened.accounts.list().unwrap()[0]["id"], user_id);
    assert!(
        reopened
            .accounts
            .resolve(admin.strip_prefix("dt_token="))
            .unwrap()
            .is_some()
    );
    reopened.shutdown().await;

    let legacy = tempfile::tempdir().unwrap();
    write_json(
        &legacy.path().join("data/user/settings/auth.json"),
        &json!({"enabled":true}),
    )
    .unwrap();
    let encoded = bcrypt::hash("old-password", 4).unwrap();
    write_json(&legacy.path().join("data/system/auth/users.json"),&json!({"old-admin":{"id":"u_existing","hash":encoded,"role":"admin","created_at":"2026-01-01T00:00:00Z"},"disabled-user":{"id":"u_disabled","hash":encoded,"role":"user","disabled":true}})).unwrap();
    let app = Application::open(legacy.path()).unwrap();
    let (user, token) = app
        .accounts
        .login("old-admin", "old-password", false)
        .unwrap();
    assert_eq!(user["id"], "u_existing");
    assert!(app.accounts.resolve(Some(&token)).unwrap().is_some());
    assert!(
        app.accounts
            .login("disabled-user", "old-password", false)
            .is_err()
    );
    app.shutdown().await;

    let flat = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(flat.path().join("data/user")).unwrap();
    std::fs::write(
        flat.path().join("data/user/auth_users.json"),
        format!(
            "{{\"z-first\":{},\"a-next\":{}}}",
            json!(encoded),
            json!(encoded)
        ),
    )
    .unwrap();
    let app = Application::open(flat.path()).unwrap();
    let users = app.accounts.list().unwrap();
    assert_eq!(
        users.iter().find(|u| u["username"] == "z-first").unwrap()["role"],
        "admin"
    );
    assert_eq!(
        users.iter().find(|u| u["username"] == "a-next").unwrap()["role"],
        "user"
    );
    app.shutdown().await;
}

#[tokio::test]
async fn model_grants_are_enforced_for_http_and_websocket_admission() {
    let server = Server::open(true).await;
    let admin = server.admin().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = format!("http://{}/v1", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        axum::serve(listener,Router::new().route("/v1/chat/completions",post(||async {
            ([("content-type","text/event-stream")],"data: {\"choices\":[{\"delta\":{\"content\":\"Private reply\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
        }))).await.unwrap();
    });
    server.json(Method::PUT,"/api/settings/catalog",&admin,json!({"catalog":{"version":1,"connections":[],"services":{"llm":{"active_profile_id":"private","active_model_id":"m","profiles":[{"id":"private","name":"Private provider","binding":"custom","base_url":provider,"api_key":"synthetic-secret","models":[{"id":"m","name":"Model","model":"synthetic-model"}]}]}}}})).await;
    let alice = server
        .json(
            Method::POST,
            "/api/auth/users",
            &admin,
            json!({"username":"alice","password":"alice-password"}),
        )
        .await;
    server
        .json(
            Method::POST,
            "/api/auth/users",
            &admin,
            json!({"username":"bob","password":"bob-password"}),
        )
        .await;
    let alice_cookie = server.activate("alice").await;
    let bob_cookie = server.activate("bob").await;
    assert_eq!(
        server
            .json(
                Method::GET,
                "/api/settings/llm-options",
                &alice_cookie,
                Value::Null
            )
            .await["options"],
        json!([])
    );
    let turn = json!({"content":"Hello","client_submission_id":"alice-1","llm_selection":{"profile_id":"assigned:private","model_id":"m"}});
    assert_eq!(
        server
            .request(Method::POST, "/api/turns", &alice_cookie, turn.clone())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let path = format!(
        "/api/multi-user/users/{}/grants",
        alice["id"].as_str().unwrap()
    );
    let mut grant = server.json(Method::GET, &path, &admin, Value::Null).await["grant"].clone();
    grant["models"]["llm"] = json!([{"profile_id":"private","model_ids":["m"]}]);
    server
        .json(Method::PUT, &path, &admin, json!({"grant":grant.clone()}))
        .await;
    let options = server
        .json(
            Method::GET,
            "/api/settings/llm-options",
            &alice_cookie,
            Value::Null,
        )
        .await;
    assert_eq!(options["options"][0]["model_id"], "m");
    assert!(
        !server
            .json(
                Method::GET,
                "/api/settings/catalog",
                &alice_cookie,
                Value::Null
            )
            .await
            .to_string()
            .contains("synthetic-secret")
    );
    let admitted = server
        .json(Method::POST, "/api/turns", &alice_cookie, turn)
        .await;
    let session = admitted["session_id"].as_str().unwrap();
    let turn = admitted["turn_id"].as_str().unwrap();
    let mut completed = false;
    for _ in 0..100 {
        let detail = server
            .json(
                Method::GET,
                &format!("/api/sessions/{session}"),
                &alice_cookie,
                Value::Null,
            )
            .await;
        if detail.to_string().contains("Private reply") {
            completed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(completed, "The granted model must produce a usable reply.");
    assert_eq!(
        server
            .request(
                Method::GET,
                &format!("/api/sessions/{session}"),
                &bob_cookie,
                Value::Null
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .request(
                Method::GET,
                &format!("/api/turns/{turn}/events"),
                &bob_cookie,
                Value::Null
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .request(
                Method::POST,
                &format!("/api/turns/{turn}/cancel"),
                &bob_cookie,
                json!({"command_id":"wrong-owner"})
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let mut bob = server.socket(&bob_cookie).await;
    bob.send(Message::Text(
        json!({"type":"subscribe_turn","protocol_version":"2.0","turn_id":turn})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let message = bob.next().await.unwrap().unwrap();
    let error: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    assert!(
        matches!(error["type"].as_str(), Some("error" | "protocol_error")),
        "{error}"
    );
    assert!(!error.to_string().contains("Private reply"));
    grant["models"]["llm"] = json!([]);
    server
        .json(Method::PUT, &path, &admin, json!({"grant":grant}))
        .await;
    let mut alice_socket = server.socket(&alice_cookie).await;
    alice_socket.send(Message::Text(json!({"type":"start_turn","protocol_version":"2.0","content":"Denied","client_submission_id":"revoked","llm_selection":{"profile_id":"assigned:private","model_id":"m"}}).to_string().into())).await.unwrap();
    let message = alice_socket.next().await.unwrap().unwrap();
    let error: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
    assert!(
        matches!(error["type"].as_str(), Some("error" | "protocol_error")),
        "{error}"
    );
    assert_eq!(error["error_code"], "start_turn_rejected");
    upstream.abort();
    server.close().await;
}

#[tokio::test]
async fn personal_keys_and_assigned_models_coexist_without_crossing_credential_owners() {
    use axum::http::HeaderMap;
    let server = Server::open(true).await;
    let admin = server.admin().await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let recorded = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = format!("http://{}/v1", listener.local_addr().unwrap());
    let upstream = tokio::spawn(async move {
        axum::serve(listener,Router::new().route("/v1/chat/completions",post(move |headers:HeaderMap|{
            let recorded=recorded.clone();async move {
                recorded.lock().unwrap().push(headers.get("authorization").unwrap().to_str().unwrap().to_owned());
                ([("content-type","text/event-stream")],"data: {\"choices\":[{\"delta\":{\"content\":\"Credential owner verified\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
            }
        }))).await.unwrap();
    });
    let catalog = |key: &str| json!({"version":1,"connections":[],"services":{"llm":{"active_profile_id":"same-id","active_model_id":"m","profiles":[{"id":"same-id","name":"Same profile name","binding":"custom","base_url":provider,"api_key":key,"models":[{"id":"m","name":"Same model","model":"synthetic"}]}]}}});
    server
        .json(
            Method::PUT,
            "/api/settings/catalog",
            &admin,
            json!({"catalog":catalog("administrator-key")}),
        )
        .await;
    let alice = server
        .json(
            Method::POST,
            "/api/auth/users",
            &admin,
            json!({"username":"alice","password":"alice-password"}),
        )
        .await;
    server
        .json(
            Method::POST,
            "/api/auth/users",
            &admin,
            json!({"username":"bob","password":"bob-password"}),
        )
        .await;
    let alice_cookie = server.activate("alice").await;
    let bob_cookie = server.activate("bob").await;
    let grant_path = format!(
        "/api/multi-user/users/{}/grants",
        alice["id"].as_str().unwrap()
    );
    let mut grant = server
        .json(Method::GET, &grant_path, &admin, Value::Null)
        .await["grant"]
        .clone();
    grant["models"]["llm"] = json!([{"profile_id":"same-id","model_ids":["m"]}]);
    server
        .json(
            Method::PUT,
            &grant_path,
            &admin,
            json!({"grant":grant.clone()}),
        )
        .await;
    server
        .json(
            Method::PUT,
            "/api/auth/profile/models",
            &alice_cookie,
            json!({"catalog":catalog("alice-personal-key")}),
        )
        .await;
    let own = server
        .json(
            Method::GET,
            "/api/auth/profile/models",
            &alice_cookie,
            Value::Null,
        )
        .await;
    assert!(!own.to_string().contains("alice-personal-key"));
    assert!(!own.to_string().contains("administrator-key"));
    assert_eq!(
        server
            .json(
                Method::GET,
                "/api/auth/profile/models",
                &bob_cookie,
                Value::Null
            )
            .await["catalog"]["services"]["llm"]["profiles"],
        json!([])
    );
    let options = server
        .json(
            Method::GET,
            "/api/settings/llm-options",
            &alice_cookie,
            Value::Null,
        )
        .await;
    assert_eq!(options["options"].as_array().unwrap().len(), 2);
    assert!(
        options["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["profile_id"] == "personal:same-id" && m["source"] == "personal")
    );
    assert!(
        options["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["profile_id"] == "assigned:same-id" && m["source"] == "admin")
    );
    // Masked values restore only Alice's own secret, despite a colliding admin id.
    server
        .json(
            Method::PUT,
            "/api/auth/profile/models",
            &alice_cookie,
            json!({"catalog":catalog("***")}),
        )
        .await;
    for (index, profile) in ["personal:same-id", "assigned:same-id"]
        .into_iter()
        .enumerate()
    {
        let receipt=server.json(Method::POST,"/api/turns",&alice_cookie,json!({"content":"Identify credential owner","client_submission_id":format!("owner-{index}"),"llm_selection":{"profile_id":profile,"model_id":"m"}})).await;
        let session = receipt["session_id"].as_str().unwrap();
        let mut completed = false;
        for _ in 0..100 {
            let detail = server
                .json(
                    Method::GET,
                    &format!("/api/sessions/{session}"),
                    &alice_cookie,
                    Value::Null,
                )
                .await;
            if detail.to_string().contains("Credential owner verified") {
                completed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(completed);
    }
    assert_eq!(
        *seen.lock().unwrap(),
        ["Bearer alice-personal-key", "Bearer administrator-key"]
    );
    assert_eq!(server.request(Method::POST,"/api/turns",&bob_cookie,json!({"content":"Denied","client_submission_id":"no-alice-access","llm_selection":{"profile_id":"personal:same-id","model_id":"m"}})).await.status(),StatusCode::BAD_REQUEST);
    assert_eq!(server.request(Method::POST,"/api/turns",&bob_cookie,json!({"content":"Denied","client_submission_id":"no-admin-access","llm_selection":{"profile_id":"assigned:same-id","model_id":"m"}})).await.status(),StatusCode::BAD_REQUEST);
    assert_eq!(
        server
            .request(
                Method::PUT,
                "/api/settings/catalog",
                &alice_cookie,
                json!({"catalog":catalog("try-to-replace-admin-key")})
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        server.app.settings.catalog().unwrap()["services"]["llm"]["profiles"][0]["api_key"],
        "administrator-key"
    );
    grant["models"]["llm"] = json!([]);
    server
        .json(Method::PUT, &grant_path, &admin, json!({"grant":grant}))
        .await;
    let options = server
        .json(
            Method::GET,
            "/api/settings/llm-options",
            &alice_cookie,
            Value::Null,
        )
        .await;
    assert_eq!(options["options"].as_array().unwrap().len(), 1);
    assert_eq!(options["options"][0]["profile_id"], "personal:same-id");
    assert_eq!(options["options"][0]["source"], "personal");
    server.json(Method::POST,"/api/turns",&alice_cookie,json!({"content":"Personal model remains available","client_submission_id":"personal-after-revocation","llm_selection":{"profile_id":"personal:same-id","model_id":"m"}})).await;
    for _ in 0..100 {
        if seen.lock().unwrap().len() >= 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        seen.lock().unwrap().last().map(String::as_str),
        Some("Bearer alice-personal-key")
    );
    assert_eq!(seen.lock().unwrap().len(), 3);
    upstream.abort();
    server.close().await;
}

#[tokio::test]
async fn guardian_permissions_material_assignment_and_revocation_are_enforced() {
    let server = Server::open(true).await;
    let admin = server.admin().await;
    let guardian = server
        .json(
            Method::POST,
            "/api/auth/users",
            &admin,
            json!({"username":"guardian","password":"guardian-password","preset":"standard"}),
        )
        .await;
    let child = server
        .json(
            Method::POST,
            "/api/auth/users",
            &admin,
            json!({"username":"child","password":"child-password","preset":"learner"}),
        )
        .await;
    let adult_cookie = server.activate("guardian").await;
    let child_cookie = server.activate("child").await;
    let child_id = child["id"].as_str().unwrap();
    let permissions = json!([
        "assign_materials",
        "manage_restrictions",
        "view_reports",
        "reset_credentials"
    ]);
    let relationship = server.json(Method::POST,"/api/multi-user/guardians",&admin,json!({"guardian_user_id":guardian["id"],"learner_user_id":child["id"],"permissions":permissions})).await;
    let report = format!("/api/multi-user/learners/{child_id}/guardian-report");
    assert_eq!(
        server
            .json(Method::GET, &report, &adult_cookie, Value::Null)
            .await["learner"]["id"],
        child["id"]
    );
    assert_eq!(
        server
            .request(
                Method::GET,
                "/api/multi-user/guardians",
                &adult_cookie,
                Value::Null
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let restrictions = format!("/api/multi-user/learners/{child_id}/restrictions");
    server.json(Method::PUT,&restrictions,&adult_cookie,json!({"age_band":"6-8","allowed_surfaces":["reading"],"allow_upload":false,"extensions":[]})).await;
    assert_eq!(
        server
            .request(Method::GET, "/api/sessions", &child_cookie, Value::Null)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let db = server.app.store.connect().unwrap();
    deeptutor_server::knowledge::ingest(
        &db,
        "",
        "approved-kb",
        &[deeptutor_server::knowledge::Document {
            path: "lesson.txt".into(),
            content: "Approved learning material".into(),
            original: b"Approved learning material".to_vec(),
        }],
        true,
    )
    .unwrap();
    let hash =
        deeptutor_server::knowledge::files(&db, "", "approved-kb").unwrap()[0]["sha256"].clone();
    let item = server
        .json(
            Method::POST,
            "/api/reading/items",
            &admin,
            json!({"kb":"approved-kb","path":"lesson.txt","hash":hash}),
        )
        .await;
    let materials = format!("/api/multi-user/learners/{child_id}/materials");
    server
        .json(
            Method::PUT,
            &materials,
            &adult_cookie,
            json!({"book_ids":[item["id"]]}),
        )
        .await;
    let items = server
        .json(
            Method::GET,
            "/api/reading/items",
            &child_cookie,
            Value::Null,
        )
        .await;
    assert_eq!(items["items"].as_array().unwrap().len(), 1);
    let copy = items["items"][0]["id"].as_str().unwrap();
    assert_eq!(
        server
            .json(
                Method::GET,
                &format!("/api/reading/items/{copy}/source"),
                &child_cookie,
                Value::Null
            )
            .await["content"],
        "Approved learning material"
    );
    server
        .json(
            Method::PUT,
            &materials,
            &adult_cookie,
            json!({"book_ids":[]}),
        )
        .await;
    assert!(
        server
            .json(
                Method::GET,
                "/api/reading/items",
                &child_cookie,
                Value::Null
            )
            .await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        server
            .request(
                Method::GET,
                &format!("/api/reading/items/{copy}"),
                &child_cookie,
                Value::Null
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    server
        .json(
            Method::POST,
            &format!("/api/multi-user/learners/{child_id}/credentials/reset"),
            &adult_cookie,
            json!({"new_password":"child-reset-password"}),
        )
        .await;
    assert_eq!(
        server
            .request(Method::GET, "/api/auth/profile", &child_cookie, Value::Null)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let reset_cookie = server
        .cookie(
            "/api/auth/login",
            json!({"username":"child","password":"child-reset-password"}),
        )
        .await;
    assert_eq!(
        server
            .json(Method::GET, "/api/auth/profile", &reset_cookie, Value::Null)
            .await["must_change_password"],
        true
    );
    let relationship_id = relationship["relationship"]["id"].as_str().unwrap();
    server
        .json(
            Method::DELETE,
            &format!("/api/multi-user/me/guardianships/{relationship_id}"),
            &adult_cookie,
            Value::Null,
        )
        .await;
    assert_eq!(
        server
            .request(Method::GET, &report, &adult_cookie, Value::Null)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        server
            .request(
                Method::PUT,
                &materials,
                &adult_cookie,
                json!({"book_ids":[item["id"]]})
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    server.close().await;
}
