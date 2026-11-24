use std::path::Path;

use base64::Engine;
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::{bootstrap, router, Config};

struct Env {
    _dir: tempfile::TempDir,
    app: axum::Router,
    cookie: String,
    state: crate::AppState,
}

impl Env {
    async fn boot() -> Self {
        let dir = tempfile::tempdir().expect("temp");
        let config = Config::for_tests(dir.path());
        let boot = bootstrap(&config).expect("bootstrap");
        let state = boot.state.clone();
        let app = router(boot.state);
        let mut env = Self { _dir: dir, app, cookie: String::new(), state };
        env.cookie = env.login("admin", "admin-password-test").await;
        env
    }

    async fn login(&self, username: &str, password: &str) -> String {
        let (status, headers, _) = self
            .call(Request::builder().method("POST").uri("/api/session").body(Body::from(format!(r#"{{"username":"{username}","password":"{password}"}}"#))).unwrap())
            .await;
        assert_eq!(status, StatusCode::OK, "login {username}");
        headers.get(header::SET_COOKIE).and_then(|v| v.to_str().ok()).unwrap().split(';').next().unwrap().to_string()
    }

    async fn call(&self, mut req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let headers = req.headers_mut();
        headers.insert("x-requested-with", "artcraft".parse().unwrap());
        if !self.cookie.is_empty() && !headers.contains_key(header::COOKIE) {
            headers.insert(header::COOKIE, self.cookie.parse().unwrap());
        }
        if !headers.contains_key(header::CONTENT_TYPE) && matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") && !path.contains("/content") && !path.ends_with("/upload") && path != "/dav" && !path.starts_with("/dav/") {
            headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        }
        let response = self.app.clone().oneshot(req).await.expect("response");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.expect("body").to_bytes().to_vec();
        (status, headers, bytes)
    }
}

fn json<'a, T: serde::Deserialize<'a>>(bytes: &'a [u8]) -> T {
    serde_json::from_slice(bytes).unwrap_or_else(|err| panic!("{err}: {}", String::from_utf8_lossy(bytes)))
}

#[tokio::test]
async fn auth_sessions_and_admin_users() {
    let env = Env::boot().await;
    let (status, _, body) = env.call(Request::builder().uri("/api/me").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let me: artcraft_cloud_types::User = json(&body);
    assert_eq!(me.username, "admin");
    assert_eq!(me.role, "admin");

    let (status, _, _) = env
        .call(Request::builder().method("POST").uri("/api/users").body(Body::from(r#"{"username":"ada","password":"ada-password","role":"user"}"#)).unwrap())
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let ada_cookie = env.login("ada", "ada-password").await;
    let mut req = Request::builder().uri("/api/users").body(Body::empty()).unwrap();
    req.headers_mut().insert(header::COOKIE, ada_cookie.parse().unwrap());
    let (status, _, _) = env.call(req).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, body) = env.call(Request::builder().method("POST").uri("/api/session").body(Body::from(r#"{"username":"ada","password":"nope-nope"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(String::from_utf8_lossy(&body).contains("invalid"));

    let (status, _, body) = env.call(Request::builder().uri("/api/users").body(Body::empty()).unwrap()).await;
    let users: Vec<artcraft_cloud_types::User> = json(&body);
    assert_eq!(status, StatusCode::OK);
    let ada = users.iter().find(|u| u.username == "ada").unwrap();
    let (status, _, _) = env.call(Request::builder().method("POST").uri(format!("/api/users/{}/disable", ada.id)).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = env.call(Request::builder().method("POST").uri("/api/session").body(Body::from(r#"{"username":"ada","password":"ada-password"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, _) = env.call(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = env.call(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn files_etag_trash_and_traversal() {
    let env = Env::boot().await;
    let (status, _, _) = env.call(Request::builder().method("POST").uri("/api/files/mkdir").body(Body::from(r#"{"path":"docs"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = env
        .call(
            Request::builder()
                .method("PUT")
                .uri("/api/files/content?path=docs/note.txt")
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from("hello"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let written: artcraft_cloud_types::WriteResult = json(&body);
    assert_eq!(written.path, "docs/note.txt");

    let (status, headers, body) = env.call(Request::builder().uri("/api/files/content?path=docs/note.txt").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"hello");
    assert!(headers.get(header::ETAG).is_some());

    let (status, _, body) = env
        .call(
            Request::builder()
                .method("PUT")
                .uri("/api/files/content?path=docs/note.txt")
                .header(header::IF_MATCH, "\"not-the-etag\"")
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from("nope"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{}", String::from_utf8_lossy(&body));
    let err: artcraft_cloud_types::ErrorBody = json(&body);
    assert_eq!(err.etag.as_deref(), Some(written.etag.as_str()));

    let (status, _, body) = env
        .call(
            Request::builder()
                .method("PUT")
                .uri("/api/files/content?path=docs/note.txt")
                .header(header::IF_MATCH, format!("\"{}\"", written.etag))
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from("hello world"))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let updated: artcraft_cloud_types::WriteResult = json(&body);
    assert_ne!(updated.etag, written.etag);

    let (status, _, _) = env.call(Request::builder().method("POST").uri("/api/files/copy").body(Body::from(r#"{"from":"docs/note.txt","to_dir":"docs","name":"copy.txt"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = env.call(Request::builder().method("POST").uri("/api/files/rename").body(Body::from(r#"{"path":"docs/copy.txt","name":"renamed.txt"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = env.call(Request::builder().method("POST").uri("/api/files/mkdir").body(Body::from(r#"{"path":"other"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = env.call(Request::builder().method("POST").uri("/api/files/move").body(Body::from(r#"{"from":"docs/renamed.txt","to_dir":"other"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = env.call(Request::builder().uri("/api/files/content?path=other/renamed.txt").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"hello world");

    let (status, _, _) = env.call(Request::builder().method("POST").uri("/api/files/delete").body(Body::from(r#"{"path":"other/renamed.txt"}"#)).unwrap()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = env.call(Request::builder().uri("/api/files/content?path=other/renamed.txt").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, body) = env.call(Request::builder().uri("/api/trash").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let trash: Vec<artcraft_cloud_types::TrashEntry> = json(&body);
    assert_eq!(trash.len(), 1);
    let (status, _, _) = env.call(Request::builder().method("POST").uri(format!("/api/trash/{}/restore", trash[0].id)).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = env.call(Request::builder().uri("/api/files/content?path=other/renamed.txt").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"hello world");

    for bad in ["/api/files/content?path=../secret", "/api/files/content?path=docs/../../etc/passwd"] {
        let (status, _, _) = env.call(Request::builder().uri(bad).body(Body::empty()).unwrap()).await;
        assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND, "{bad} -> {status}");
    }
}

#[tokio::test]
async fn sharing_permissions_and_public_links() {
    let env = Env::boot().await;
    env.call(Request::builder().method("PUT").uri("/api/files/content?path=secret.txt").header(header::CONTENT_TYPE, "application/octet-stream").body(Body::from("secret")).unwrap()).await;
    env.call(Request::builder().method("POST").uri("/api/users").body(Body::from(r#"{"username":"bea","password":"bea-password"}"#)).unwrap()).await;
    env.call(Request::builder().method("POST").uri("/api/users").body(Body::from(r#"{"username":"cy","password":"cy-password-ok"}"#)).unwrap()).await;

    let (status, _, body) = env
        .call(Request::builder().method("POST").uri("/api/shares").body(Body::from(r#"{"path":"secret.txt","grantee":"bea","permission":"view","public":false}"#)).unwrap())
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let view: artcraft_cloud_types::Share = json(&body);

    let bea = env.login("bea", "bea-password").await;
    let mut read = Request::builder().uri(format!("/api/shares/{}/content", view.id)).body(Body::empty()).unwrap();
    read.headers_mut().insert(header::COOKIE, bea.parse().unwrap());
    let (status, _, body) = env.call(read).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"secret");

    let mut write = Request::builder()
        .method("PUT")
        .uri(format!("/api/shares/{}/content", view.id))
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from("changed"))
        .unwrap();
    write.headers_mut().insert(header::COOKIE, bea.parse().unwrap());
    let (status, _, _) = env.call(write).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, body) = env
        .call(Request::builder().method("POST").uri("/api/shares").body(Body::from(r#"{"path":"secret.txt","grantee":"cy","permission":"edit","public":false}"#)).unwrap())
        .await;
    let edit: artcraft_cloud_types::Share = json(&body);
    assert_eq!(status, StatusCode::OK);
    let cy = env.login("cy", "cy-password-ok").await;
    let (_status, _, body) = env.call(Request::builder().uri("/api/files/content?path=secret.txt").body(Body::empty()).unwrap()).await;
    assert_eq!(body, b"secret");
    let mut write = Request::builder()
        .method("PUT")
        .uri(format!("/api/shares/{}/content?path=", edit.id))
        .header(header::IF_MATCH, format!("\"{}\"", {
            let (_, headers, _) = {
                let mut read = Request::builder().uri(format!("/api/shares/{}/content", edit.id)).body(Body::empty()).unwrap();
                read.headers_mut().insert(header::COOKIE, cy.parse().unwrap());
                env.call(read).await
            };
            headers.get(header::ETAG).unwrap().to_str().unwrap().trim_matches('"').to_string()
        }))
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from("edited"))
        .unwrap();
    write.headers_mut().insert(header::COOKIE, cy.parse().unwrap());
    let (status, _, body) = env.call(write).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let _ = status;

    let (status, _, body) = env.call(Request::builder().uri("/api/files/content?path=secret.txt").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"edited");

    let mut stranger = Request::builder().uri(format!("/api/shares/{}/content", edit.id)).body(Body::empty()).unwrap();
    stranger.headers_mut().insert(header::COOKIE, bea.parse().unwrap());
    let (status, _, _) = env.call(stranger).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, body) = env
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/shares")
                .body(Body::from(r#"{"path":"secret.txt","permission":"view","public":true,"password":"link-secret","expires_at":4102444800}"#))
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let link: artcraft_cloud_types::Share = json(&body);
    let token = link.token.unwrap();
    let mut open = Request::builder().uri(format!("/api/public/{token}")).body(Body::empty()).unwrap();
    open.headers_mut().remove(header::COOKIE);
    // call() injects the admin cookie. Build a fresh request through the app.
    let response = env.app.clone().oneshot(Request::builder().uri(format!("/api/public/{token}")).body(Body::empty()).unwrap()).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let view: artcraft_cloud_types::PublicShareView = json(&bytes);
    assert!(view.needs_password);

    let past = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/shares")
                .header("x-requested-with", "artcraft")
                .header(header::COOKIE, env.cookie.parse::<axum::http::HeaderValue>().unwrap())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"path":"secret.txt","permission":"view","public":true,"expires_at":10}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(past.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn webdav_round_trip() {
    let env = Env::boot().await;
    let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("admin:admin-password-test"));
    let (status, _headers) = env.app.clone().oneshot(Request::builder().uri("/dav/").body(Body::empty()).unwrap()).await.unwrap().into_parts_status();
    let _ = status;

    let response = env.app.clone().oneshot(Request::builder().method(Method::from_bytes(b"PROPFIND").unwrap()).uri("/dav/").header("depth", "0").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().get(header::WWW_AUTHENTICATE).is_some());

    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::from_bytes(b"MKCOL").unwrap())
                .uri("/dav/folder")
                .header(header::AUTHORIZATION, &auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/dav/folder/a.txt")
                .header(header::AUTHORIZATION, &auth)
                .body(Body::from("dav"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let response = env.app.clone().oneshot(Request::builder().uri("/dav/folder/a.txt").header(header::AUTHORIZATION, &auth).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"dav");

    // A name that starts with "dav" keeps it.
    let response = env.app.clone().oneshot(Request::builder().method("PUT").uri("/dav/dav-notes.txt").header(header::AUTHORIZATION, &auth).body(Body::from("notes")).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let (status, _, body) = env.call(Request::builder().uri("/api/files/content?path=dav-notes.txt").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body[..], b"notes");

    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::from_bytes(b"PROPFIND").unwrap())
                .uri("/dav/folder/")
                .header(header::AUTHORIZATION, &auth)
                .header("depth", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let xml = String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(xml.contains("a.txt"), "{xml}");

    let response = env
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::from_bytes(b"MOVE").unwrap())
                .uri("/dav/folder/a.txt")
                .header(header::AUTHORIZATION, &auth)
                .header("destination", "http://localhost/dav/moved.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = env.app.clone().oneshot(Request::builder().uri("/dav/moved.txt").header(header::AUTHORIZATION, &auth).body(Body::empty()).unwrap()).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"dav");

    let response = env
        .app
        .clone()
        .oneshot(Request::builder().method("DELETE").uri("/dav/moved.txt").header(header::AUTHORIZATION, &auth).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = env.app.clone().oneshot(Request::builder().uri("/dav/moved.txt").header(header::AUTHORIZATION, &auth).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

trait StatusParts {
    fn into_parts_status(self) -> (StatusCode, axum::http::HeaderMap);
}

impl StatusParts for axum::response::Response {
    fn into_parts_status(self) -> (StatusCode, axum::http::HeaderMap) {
        (self.status(), self.headers().clone())
    }
}

#[test]
fn oidc_configuration_refuses_to_boot() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::for_tests(dir.path());
    config.oidc_issuer = Some("https://idp.example".into());
    assert!(bootstrap(&config).is_err());
}

#[test]
fn home_stays_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::for_tests(dir.path());
    let boot = bootstrap(&config).unwrap();
    assert!(Path::new(dir.path()).join("db.sqlite").is_file() || dir.path().join("db.sqlite").exists());
    let _ = boot;
}

fn req(method: &str, uri: &str, body: &str, cookie: Option<&str>) -> Request<Body> {
    let mut req = Request::builder().method(method).uri(uri).body(Body::from(body.to_string())).unwrap();
    if let Some(cookie) = cookie {
        req.headers_mut().insert(header::COOKIE, cookie.parse().unwrap());
    }
    req
}

async fn make_user(env: &Env, name: &str, role: &str) -> i64 {
    let (status, _, body) = env
        .call(req("POST", "/api/users", &format!(r#"{{"username":"{name}","password":"{name}-password","role":"{role}"}}"#), None))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&body));
    json::<artcraft_cloud_types::User>(&body).id
}

#[tokio::test]
async fn the_first_admin_is_super_and_alone_manages_admins() {
    let env = Env::boot().await;
    let (_, _, body) = env.call(req("GET", "/api/me", "", None)).await;
    let me: artcraft_cloud_types::User = json(&body);
    assert!(me.super_admin, "the bootstrap admin is the super admin");

    let bob = make_user(&env, "bob", "admin").await;
    let carol = make_user(&env, "carol", "user").await;
    let dave = make_user(&env, "dave", "user").await;
    let bob_cookie = env.login("bob", "bob-password").await;
    let carol_cookie = env.login("carol", "carol-password").await;

    // People list works for every admin, super or not (the People tab's first request).
    let (status, _, body) = env.call(req("GET", "/api/users", "", Some(&bob_cookie))).await;
    assert_eq!(status, StatusCode::OK);
    let users: Vec<artcraft_cloud_types::User> = json(&body);
    assert_eq!(users.len(), 4);
    assert_eq!(users.iter().filter(|u| u.super_admin).count(), 1);
    assert!(!users.iter().find(|u| u.username == "bob").unwrap().super_admin);

    // A plain admin can create users but not admins, and cannot promote, demote or disable an admin.
    let (status, _, _) = env.call(req("POST", "/api/users", r#"{"username":"erin","password":"erin-password","role":"user"}"#, Some(&bob_cookie))).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, _) = env.call(req("POST", "/api/users", r#"{"username":"frank","password":"frank-password","role":"admin"}"#, Some(&bob_cookie))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = env.call(req("POST", &format!("/api/users/{carol}/role"), r#"{"role":"admin"}"#, Some(&bob_cookie))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = env.call(req("POST", &format!("/api/users/{}/role", me.id), r#"{"role":"user"}"#, Some(&bob_cookie))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = env.call(req("POST", &format!("/api/users/{}/disable", me.id), "", Some(&bob_cookie))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = env.call(req("POST", &format!("/api/users/{}/password", me.id), r#"{"password":"taken-over-123"}"#, Some(&bob_cookie))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // ...but can still disable a regular user.
    let (status, _, _) = env.call(req("POST", &format!("/api/users/{dave}/disable"), "", Some(&bob_cookie))).await;
    assert_eq!(status, StatusCode::OK);

    // A regular user can do none of it.
    let (status, _, _) = env.call(req("POST", "/api/users", r#"{"username":"gus","password":"gus-password"}"#, Some(&carol_cookie))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = env.call(req("POST", &format!("/api/users/{carol}/role"), r#"{"role":"admin"}"#, Some(&carol_cookie))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The super admin promotes and demotes.
    let (status, _, body) = env.call(req("POST", &format!("/api/users/{carol}/role"), r#"{"role":"admin"}"#, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json::<artcraft_cloud_types::User>(&body).role, "admin");
    let (status, _, _) = env.call(req("GET", "/api/users", "", Some(&carol_cookie))).await;
    assert_eq!(status, StatusCode::OK, "the role change applies to the existing session");
    let (status, _, body) = env.call(req("POST", &format!("/api/users/{bob}/role"), r#"{"role":"user"}"#, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json::<artcraft_cloud_types::User>(&body).role, "user");
    let (status, _, _) = env.call(req("GET", "/api/users", "", Some(&bob_cookie))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = env.call(req("POST", &format!("/api/users/{carol}/role"), r#"{"role":"owner"}"#, None)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The super admin cannot be demoted or disabled, not even by itself.
    let (status, _, _) = env.call(req("POST", &format!("/api/users/{}/role", me.id), r#"{"role":"user"}"#, None)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = env.call(req("POST", &format!("/api/users/{}/disable", me.id), "", Some(&carol_cookie))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn groups_share_with_every_member() {
    let env = Env::boot().await;
    make_user(&env, "ann", "user").await;
    make_user(&env, "ben", "user").await;
    let ann = env.login("ann", "ann-password").await;
    let ben = env.login("ben", "ben-password").await;

    let (status, _, body) = env.call(req("POST", "/api/groups", r#"{"name":"Finance"}"#, None)).await;
    assert_eq!(status, StatusCode::CREATED);
    let group: artcraft_cloud_types::Group = json(&body);
    let (status, _, _) = env.call(req("POST", "/api/groups", r#"{"name":"finance"}"#, None)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _, _) = env.call(req("POST", "/api/groups", r#"{"name":"Nope"}"#, Some(&ann))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "only admins manage groups");
    let (status, _, _) = env.call(req("POST", &format!("/api/groups/{}/members", group.id), r#"{"username":"ann"}"#, None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Everyone can read the directory for the share picker; it leaves out the caller.
    let (status, _, body) = env.call(req("GET", "/api/directory", "", Some(&ben))).await;
    assert_eq!(status, StatusCode::OK);
    let dir: artcraft_cloud_types::Directory = json(&body);
    assert!(dir.users.iter().any(|u| u.username == "ann") && !dir.users.iter().any(|u| u.username == "ben"));
    assert_eq!(dir.groups, vec![artcraft_cloud_types::DirectoryGroup { name: "Finance".into(), members: 1 }]);

    let (status, _, _) = env.call(Request::builder().method("PUT").uri("/api/files/content?path=report.txt").body(Body::from("q4")).unwrap()).await;
    assert!(status.is_success());
    let (status, _, body) = env.call(req("POST", "/api/shares", r#"{"path":"report.txt","permission":"view","public":false,"group":"Finance"}"#, None)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let share: artcraft_cloud_types::Share = json(&body);
    assert_eq!(share.group.as_deref(), Some("Finance"));

    let (_, _, body) = env.call(req("GET", "/api/shares?box=in", "", Some(&ann))).await;
    let incoming: Vec<artcraft_cloud_types::Share> = json(&body);
    assert_eq!(incoming.len(), 1, "a member sees the group share");
    let (status, _, body) = env.call(req("GET", &format!("/api/shares/{}/content", share.id), "", Some(&ann))).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, b"q4".as_slice()));

    let (_, _, body) = env.call(req("GET", "/api/shares?box=in", "", Some(&ben))).await;
    assert!(json::<Vec<artcraft_cloud_types::Share>>(&body).is_empty());
    let (status, _, _) = env.call(req("GET", &format!("/api/shares/{}/content", share.id), "", Some(&ben))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a non-member cannot read it");

    // Leaving the group ends the access.
    let (status, _, _) = env.call(req("DELETE", &format!("/api/groups/{}/members/ann", group.id), "", None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = env.call(req("GET", &format!("/api/shares/{}/content", share.id), "", Some(&ann))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn app_settings_are_kept_per_user_and_app() {
    let env = Env::boot().await;
    make_user(&env, "ivy", "user").await;
    let ivy = env.login("ivy", "ivy-password").await;
    let (status, _, body) = env.call(req("GET", "/api/settings/wordcraft", "", None)).await;
    assert_eq!((status, body.as_slice()), (StatusCode::OK, b"{}".as_slice()));
    let prefs = r#"{"wordcraft.ui":"{\"author\":\"GH\"}"}"#;
    let (status, _, _) = env.call(req("PUT", "/api/settings/wordcraft", prefs, None)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, body) = env.call(req("GET", "/api/settings/wordcraft", "", None)).await;
    assert_eq!(json::<serde_json::Value>(&body)["wordcraft.ui"], "{\"author\":\"GH\"}");
    let (_, _, body) = env.call(req("GET", "/api/settings/wordcraft", "", Some(&ivy))).await;
    assert_eq!(body, b"{}", "settings are per user");
    let (_, _, body) = env.call(req("GET", "/api/settings/gridcraft", "", None)).await;
    assert_eq!(body, b"{}", "and per app");
    let (status, _, _) = env.call(req("PUT", "/api/settings/notanapp", "{}", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = env.call(req("PUT", "/api/settings/gridcraft", "[1,2]", None)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let big = format!(r#"{{"k":"{}"}}"#, "x".repeat(super::http::users::SETTINGS_LIMIT));
    let (status, _, _) = env.call(req("PUT", "/api/settings/gridcraft", &big, None)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn preferences_override_every_apps_options() {
    let env = Env::boot().await;
    make_user(&env, "ivy", "user").await;
    let ivy = env.login("ivy", "ivy-password").await;

    // Defaults until something is chosen.
    let (status, _, body) = env.call(req("GET", "/api/preferences", "", None)).await;
    assert_eq!(status, StatusCode::OK);
    let prefs: artcraft_cloud_types::prefs::Preferences = json(&body);
    assert_eq!(prefs, artcraft_cloud_types::prefs::Preferences::default());

    // WordCraft already has options of its own; GridCraft has none yet.
    let word = r#"{"wordcraft.ui":"{\"theme\":\"light\",\"language\":\"fr\",\"author\":\"Old\",\"tab\":\"Insert\"}"}"#;
    env.call(req("PUT", "/api/settings/wordcraft", word, None)).await;

    let next = r#"{"theme":"dark","language":"de","author":"Gabriel Heiler","initials":"","units":"centimeters","animations":"off","autosave":true,"junk":1}"#;
    let (status, _, body) = env.call(req("PUT", "/api/preferences", next, None)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let stored: artcraft_cloud_types::prefs::Preferences = json(&body);
    assert_eq!((stored.theme.as_str(), stored.language.as_str()), ("dark", "de"));

    let app = |body: &[u8], key: &str| -> serde_json::Value {
        let map: serde_json::Value = json(body);
        serde_json::from_str(map[key].as_str().unwrap()).unwrap()
    };
    let (_, _, body) = env.call(req("GET", "/api/settings/wordcraft", "", None)).await;
    let ui = app(&body, "wordcraft.ui");
    assert_eq!(ui["theme"], "dark");
    assert_eq!(ui["language"], "de");
    assert_eq!(ui["author"], "Gabriel Heiler");
    assert_eq!(ui["initials"], "GH");
    assert_eq!(ui["editing"]["measurementUnits"], "centimeters");
    assert_eq!(ui["tab"], "Insert", "the app's other options are kept");
    let (_, _, body) = env.call(req("GET", "/api/settings/gridcraft", "", None)).await;
    assert_eq!(app(&body, "gridcraft.ui")["dark"], true);
    assert_eq!(app(&body, "gridcraft.prefs")["userName"], "Gabriel Heiler");
    let (_, _, body) = env.call(req("GET", "/api/settings/figcraft", "", None)).await;
    assert_eq!(json::<serde_json::Value>(&body)["figcraft.userName"], "Gabriel Heiler");

    // An option changed inside an app after that is not overwritten by an unrelated change.
    let word = r#"{"wordcraft.ui":"{\"theme\":\"light\",\"language\":\"de\"}"}"#;
    env.call(req("PUT", "/api/settings/wordcraft", word, None)).await;
    let next = next.replace(r#""animations":"off""#, r#""animations":"on""#);
    env.call(req("PUT", "/api/preferences", &next, None)).await;
    let (_, _, body) = env.call(req("GET", "/api/settings/wordcraft", "", None)).await;
    let ui = app(&body, "wordcraft.ui");
    assert_eq!((ui["theme"].as_str(), ui["animations"].as_str()), (Some("light"), Some("on")), "only the changed setting is overridden");

    // Per user, unknown values fall back, and the store is not reachable as an app.
    let (_, _, body) = env.call(req("GET", "/api/preferences", "", Some(&ivy))).await;
    assert_eq!(json::<artcraft_cloud_types::prefs::Preferences>(&body).theme, "system");
    let (_, _, body) = env.call(req("GET", "/api/settings/wordcraft", "", Some(&ivy))).await;
    assert_eq!(body, b"{}", "another user's apps are untouched");
    let (_, _, body) = env.call(req("PUT", "/api/preferences", r#"{"theme":"neon","language":"xx"}"#, Some(&ivy))).await;
    let odd: artcraft_cloud_types::prefs::Preferences = json(&body);
    assert_eq!((odd.theme.as_str(), odd.language.as_str()), ("system", "auto"));
    let (status, _, _) = env.call(req("GET", "/api/settings/preferences", "", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = env.call(req("PUT", "/api/preferences", "[]", None)).await;
    assert!(status.is_client_error());
    let (status, _, _) = env.call(req("GET", "/api/preferences", "", Some("artcraft_session=nope"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// A stand-in for Stalwart's management API and JMAP endpoints, on a real port.
mod fake_stalwart {
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use base64::Engine;
    use serde_json::{json, Value};

    pub const ADMIN: (&str, &str) = ("admin@example.test", "stalwart-admin-pw");

    pub struct Acct {
        pub id: String,
        pub name: String,
        pub domain: String,
        pub password: String,
        /// (credential id, secret)
        pub apps: Vec<(String, String)>,
        pub permissions: Value,
    }

    impl Acct {
        pub fn suspended(&self) -> bool {
            self.permissions["disabledPermissions"]["authenticate"] == true
        }
    }

    #[derive(Default)]
    pub struct Fake {
        pub domains: Vec<(String, String)>,
        pub accounts: Vec<Acct>,
        pub api_keys: Vec<String>,
        /// Password updates fail as if the server were down.
        pub fail_updates: bool,
        /// Mailbox sign-ins, by kind of credential.
        pub app_logins: u32,
        pub password_logins: u32,
        next: u32,
    }

    impl Fake {
        pub fn account(&self, name: &str) -> &Acct {
            self.accounts.iter().find(|a| a.name == name).expect("account")
        }
    }

    pub type Shared = Arc<Mutex<Fake>>;

    fn basic(headers: &HeaderMap) -> Option<(String, String)> {
        let value = headers.get("authorization")?.to_str().ok()?.strip_prefix("Basic ")?;
        let decoded = String::from_utf8(base64::engine::general_purpose::STANDARD.decode(value).ok()?).ok()?;
        let (user, pass) = decoded.split_once(':')?;
        Some((user.to_string(), pass.to_string()))
    }

    fn is_admin(fake: &Fake, headers: &HeaderMap) -> bool {
        let bearer = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
        basic(headers).is_some_and(|(u, p)| u == ADMIN.0 && p == ADMIN.1) || bearer.is_some_and(|key| fake.api_keys.iter().any(|k| k == key))
    }

    /// The mailbox signing in, as `local@domain`.
    fn user(fake: &mut Fake, headers: &HeaderMap) -> Option<String> {
        let (u, p) = basic(headers)?;
        let (app, password) = fake.accounts.iter().find_map(|a| {
            let dname = &fake.domains.iter().find(|(id, _)| *id == a.domain)?.1;
            (format!("{}@{dname}", a.name) == u && !a.suspended()).then(|| (a.apps.iter().any(|(_, s)| *s == p), a.password == p))
        })?;
        if app {
            fake.app_logins += 1;
        } else if password {
            fake.password_logins += 1;
        }
        (app || password).then_some(u)
    }

    async fn account(State(fake): State<Shared>, headers: HeaderMap) -> impl IntoResponse {
        if !is_admin(&fake.lock().unwrap(), &headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Json(json!({"permissions": crate::stalwart::NEEDED, "edition": "community", "locale": "en-US"})).into_response()
    }

    async fn session(State(fake): State<Shared>, headers: HeaderMap) -> impl IntoResponse {
        let mut fake = fake.lock().unwrap();
        let Some(user) = user(&mut fake, &headers) else { return StatusCode::UNAUTHORIZED.into_response() };
        Json(json!({
            "username": user,
            "apiUrl": "http://mail.internal:8080/jmap/",
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
            "uploadUrl": "/jmap/upload/{accountId}/",
            "eventSourceUrl": "/jmap/eventsource/",
            "capabilities": {"urn:ietf:params:jmap:core": {}, "urn:ietf:params:jmap:websocket": {"url": "ws/jmap/ws"}},
            "accounts": {}, "primaryAccounts": {}, "state": "s1"
        }))
        .into_response()
    }

    async fn api(State(fake): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
        let mut fake = fake.lock().unwrap();
        if !is_admin(&fake, &headers) {
            return match user(&mut fake, &headers) {
                Some(user) => Json(json!({"methodResponses": [["Mailbox/get", {"list": [], "who": format!("user:{user}")}, "0"]]})).into_response(),
                None => StatusCode::UNAUTHORIZED.into_response(),
            };
        }
        let mut out = Vec::new();
        for call in body["methodCalls"].as_array().unwrap() {
            let (name, args, tag) = (call[0].as_str().unwrap(), &call[1], call[2].clone());
            let reply = match name {
                "x:Domain/query" => json!({"ids": fake.domains.iter().map(|d| d.0.clone()).collect::<Vec<_>>()}),
                "x:Domain/get" => json!({"list": fake.domains.iter().map(|(id, name)| json!({
                    "id": id, "name": name,
                    "dnsZoneFile": format!("{name}. IN MX 10 mail.{name}.\n{name}. IN TXT \"v=spf1 mx -all\"\nv1._domainkey.{name}. IN TXT (\n \"v=DKIM1; p=AB\"\n \"CD\"\n)\n_dmarc.{name}. IN TXT \"v=DMARC1; p=reject\"\n")
                })).collect::<Vec<_>>()}),
                "x:Domain/set" => {
                    let name = args["create"]["d"]["name"].as_str().unwrap().to_string();
                    fake.next += 1;
                    let id = format!("d{}", fake.next);
                    fake.domains.push((id.clone(), name));
                    json!({"created": {"d": {"id": id}}})
                }
                "x:Account/query" => {
                    let (n, d) = (args["filter"]["name"].as_str().unwrap_or(""), args["filter"]["domainId"].as_str().unwrap_or(""));
                    json!({"ids": fake.accounts.iter().filter(|a| a.name == n && a.domain == d).map(|a| a.id.clone()).collect::<Vec<_>>()})
                }
                "x:Account/get" => json!({"list": fake.accounts.iter().filter(|a| args["ids"].as_array().is_none_or(|ids| ids.iter().any(|i| *i == a.id))).map(|a| json!({
                    "id": a.id, "name": a.name, "domainId": a.domain, "permissions": a.permissions,
                    "credentials": {"0": {"@type": "Password", "credentialId": "a", "secret": "****"}, "1": {"@type": "AppPassword", "credentialId": "b", "secret": "****"}}
                })).collect::<Vec<_>>()}),
                "x:Account/set" if args.get("create").is_some() => {
                    let a = &args["create"]["u"];
                    assert_eq!(a["@type"], "User");
                    let password = a["credentials"]["0"]["secret"].as_str().unwrap().to_string();
                    assert!(password.len() >= 8);
                    fake.next += 1;
                    let id = format!("a{}", fake.next);
                    fake.accounts.push(Acct {
                        id: id.clone(),
                        name: a["name"].as_str().unwrap().into(),
                        domain: a["domainId"].as_str().unwrap().into(),
                        password,
                        apps: Vec::new(),
                        permissions: json!({"@type": "Inherit"}),
                    });
                    json!({"created": {"u": {"id": id}}})
                }
                "x:Account/set" if args.get("update").is_some() => {
                    if fake.fail_updates {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    let (id, patch) = args["update"].as_object().unwrap().iter().next().unwrap();
                    let acct = fake.accounts.iter_mut().find(|a| a.id == *id).unwrap();
                    for (key, value) in patch.as_object().unwrap() {
                        if key == "credentials/0/secret" {
                            acct.password = value.as_str().unwrap().to_string();
                        } else if key == "permissions" {
                            acct.permissions = value.clone();
                        } else if let Some(p) = key.strip_prefix("permissions/disabledPermissions/") {
                            let list = acct.permissions["disabledPermissions"].as_object_mut().expect("patches a Merge list");
                            if value.is_null() {
                                list.remove(p);
                            } else {
                                list.insert(p.to_string(), value.clone());
                            }
                        } else {
                            panic!("unexpected patch {key}");
                        }
                    }
                    json!({"updated": {id.clone(): null}})
                }
                "x:Account/set" => {
                    let id = args["destroy"][0].as_str().unwrap().to_string();
                    fake.accounts.retain(|a| a.id != id);
                    json!({"destroyed": [id]})
                }
                "x:AppPassword/set" => {
                    let account = args["accountId"].as_str().expect("for a named account").to_string();
                    fake.next += 1;
                    let n = fake.next;
                    let acct = fake.accounts.iter_mut().find(|a| a.id == account).unwrap();
                    if let Some(ids) = args["destroy"].as_array() {
                        acct.apps.retain(|(id, _)| !ids.iter().any(|i| i == id));
                        json!({"destroyed": ids})
                    } else {
                        assert_eq!(args["create"]["p"]["permissions"]["@type"], "Inherit");
                        let (id, secret) = (format!("ap{n}"), format!("app_secret_{n}"));
                        acct.apps.push((id.clone(), secret.clone()));
                        json!({"created": {"p": {"id": id, "secret": secret}}})
                    }
                }
                "x:ApiKey/set" => {
                    fake.next += 1;
                    let key = format!("API_key_{}", fake.next);
                    fake.api_keys.push(key.clone());
                    json!({"created": {"k": {"id": "k", "secret": key}}})
                }
                other => panic!("unexpected {other}"),
            };
            out.push(json!([name, reply, tag]));
        }
        Json(json!({"methodResponses": out})).into_response()
    }

    /// CalDAV: answers as the mailbox signing in, naming the request and with the server's own
    /// paths in the reply (`/dav/cal/…`, as Stalwart does).
    async fn caldav(State(fake): State<Shared>, method: axum::http::Method, uri: axum::http::Uri, headers: HeaderMap) -> impl IntoResponse {
        let mut fake = fake.lock().unwrap();
        let Some(user) = user(&mut fake, &headers) else { return StatusCode::UNAUTHORIZED.into_response() };
        let depth = headers.get("depth").and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
        let path = uri.path().to_string();
        let body = format!(
            "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>{path}default/</d:href><d:propstat><d:prop><d:displayname>{method} {path} depth={depth} user={user}</d:displayname></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"
        );
        (StatusCode::MULTI_STATUS, [("content-type", "application/xml; charset=utf-8"), ("etag", "\"7\"")], body).into_response()
    }

    pub async fn start() -> (String, Shared) {
        let shared: Shared = Arc::default();
        let app = Router::new()
            .route("/api/account", get(account))
            .route("/jmap/session", get(session))
            .route("/jmap/", post(api))
            .route("/dav/cal/{*rest}", axum::routing::any(caldav))
            .with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, shared)
    }
}

#[tokio::test]
async fn mail_server_domains_mailboxes_and_the_jmap_proxy() {
    let env = Env::boot().await;
    let (url, fake) = fake_stalwart::start().await;
    let get = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();
    let send = |method: &str, uri: &str, body: &str| Request::builder().method(method).uri(uri).body(Body::from(body.to_string())).unwrap();
    let as_user = |mut req: Request<Body>, cookie: &str| {
        req.headers_mut().insert(header::COOKIE, cookie.parse().unwrap());
        req
    };

    let (status, _, body) = env.call(get("/api/mail/config")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json::<artcraft_cloud_types::MailConfig>(&body).base_url, "");
    let (status, _, _) = env.call(send("POST", "/api/mail/test", "")).await;
    assert_eq!(status, StatusCode::CONFLICT, "nothing configured yet");

    let config = format!(r#"{{"base_url":"{url}/","auth":"basic","username":"{}","secret":"{}"}}"#, fake_stalwart::ADMIN.0, fake_stalwart::ADMIN.1);
    let (status, _, body) = env.call(send("PUT", "/api/mail/config", &config)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let saved: artcraft_cloud_types::MailConfig = json(&body);
    assert_eq!((saved.base_url.as_str(), saved.has_secret), (url.as_str(), true));
    let (_, _, body) = env.call(get("/api/mail/config")).await;
    assert!(!String::from_utf8_lossy(&body).contains(fake_stalwart::ADMIN.1), "the secret never goes back");
    for file in ["db.sqlite", "db.sqlite-wal"] {
        let bytes = std::fs::read(env._dir.path().join(file)).unwrap_or_default();
        assert!(!bytes.windows(fake_stalwart::ADMIN.1.len()).any(|w| w == fake_stalwart::ADMIN.1.as_bytes()), "sealed at rest ({file})");
    }
    // A new address without a secret keeps the stored one.
    let (status, _, _) = env.call(send("PUT", "/api/mail/config", &format!(r#"{{"base_url":"{url}","auth":"basic","username":"{}"}}"#, fake_stalwart::ADMIN.0))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = env.call(send("POST", "/api/mail/test", "")).await;
    let test: artcraft_cloud_types::MailTest = json(&body);
    assert!(status == StatusCode::OK && test.ok && test.can_manage, "{test:?}");
    // The recommended way: the admin login makes an API key once, and only the key is kept.
    let (status, _, body) = env.call(send("PUT", "/api/mail/config", &format!(r#"{{"base_url":"{url}","auth":"basic","username":"{}","create_api_key":true,"public_host":"Mail.Example.test"}}"#, fake_stalwart::ADMIN.0))).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let saved: artcraft_cloud_types::MailConfig = json(&body);
    assert_eq!((saved.auth.as_str(), saved.username.as_str(), saved.public_host.as_str()), ("bearer", "", "mail.example.test"));
    assert_eq!(fake.lock().unwrap().api_keys.len(), 1);
    let (_, _, body) = env.call(send("POST", "/api/mail/test", "")).await;
    assert!(json::<artcraft_cloud_types::MailTest>(&body).ok, "the API key works");

    let (status, _, body) = env.call(send("POST", "/api/mail/domains", r#"{"name":"Example.TEST"}"#)).await;
    assert_eq!(status, StatusCode::CREATED);
    let domain: artcraft_cloud_types::MailDomain = json(&body);
    assert_eq!(domain.name, "example.test");
    let purposes: Vec<&str> = domain.records.iter().map(|r| r.purpose.as_str()).collect();
    assert_eq!(purposes, ["MX", "SPF", "DKIM", "DMARC"]);
    assert_eq!(domain.records[2].value, "v=DKIM1; p=ABCD");
    let (status, _, _) = env.call(send("POST", "/api/mail/domains", r#"{"name":"example.test"}"#)).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // A user whose address is on a served domain gets a mailbox; another domain gets the email only.
    let (status, _, body) = env.call(send("POST", "/api/users", r#"{"username":"ada","password":"ada-password","email":"Ada@Example.test","create_mailbox":true}"#)).await;
    assert_eq!(status, StatusCode::CREATED);
    let ada: artcraft_cloud_types::User = json(&body);
    assert_eq!((ada.email.as_deref(), ada.mailbox), (Some("ada@example.test"), true));
    assert_eq!(ada.mail_sync, "synced");
    {
        let fake = fake.lock().unwrap();
        let account = fake.account("ada");
        assert_eq!(account.password, "ada-password", "the mailbox password is the artcraftCloud password");
        assert_eq!(account.apps.len(), 1, "and the proxy has an app password");
    }
    let (status, _, body) = env.call(send("POST", "/api/users", r#"{"username":"bob","password":"bob-password","email":"bob@elsewhere.org","create_mailbox":true}"#)).await;
    assert_eq!(status, StatusCode::CREATED);
    let bob: artcraft_cloud_types::User = json(&body);
    assert_eq!((bob.email.as_deref(), bob.mailbox), (Some("bob@elsewhere.org"), false));
    let (status, _, body) = env.call(send("PUT", &format!("/api/users/{}/email", bob.id), r#"{"email":"bob@elsewhere.org","create_mailbox":true}"#)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(String::from_utf8_lossy(&body).contains("does not serve elsewhere.org"));
    let (status, _, _) = env.call(send("PUT", &format!("/api/users/{}/email", bob.id), r#"{"email":"ADA@example.test"}"#)).await;
    assert_eq!(status, StatusCode::CONFLICT, "emails are unique");
    let (status, _, _) = env.call(send("PUT", &format!("/api/users/{}/email", ada.id), r#"{"email":"ada2@example.test"}"#)).await;
    assert_eq!(status, StatusCode::CONFLICT, "a linked mailbox keeps its address");
    let (_, _, body) = env.call(get("/api/users")).await;
    let users: Vec<artcraft_cloud_types::User> = json(&body);
    assert!(users.iter().any(|u| u.username == "ada" && u.mailbox && u.email.as_deref() == Some("ada@example.test")));

    // MailCraft: the hook names the mailbox, the token opens the proxy as that mailbox.
    let ada_cookie = env.login("ada", "ada-password").await;
    let bob_cookie = env.login("bob", "bob-password").await;
    let (_, _, body) = env.call(as_user(get("/api/mail/mailcraft.js"), &ada_cookie)).await;
    let hook = String::from_utf8_lossy(&body).to_string();
    assert!(hook.contains("window.mailcraftJmap") && hook.contains("\"ada@example.test\"") && hook.contains("/jmap/session"), "{hook}");
    let (_, _, body) = env.call(as_user(get("/api/mail/mailcraft.js"), &bob_cookie)).await;
    assert!(String::from_utf8_lossy(&body).contains("no mailbox"));
    let (status, _, _) = env.call(as_user(send("POST", "/api/mail/token", ""), &bob_cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, body) = env.call(as_user(send("POST", "/api/mail/token", ""), &ada_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let token = json::<serde_json::Value>(&body)["token"].as_str().unwrap().to_string();
    let bearer = |mut req: Request<Body>, token: &str| {
        req.headers_mut().insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        req.headers_mut().insert(header::COOKIE, "artcraft_session=none".parse().unwrap());
        req
    };
    let (status, headers, _) = env.call(bearer(get("/jmap/session"), "mc1.1.2.3")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.contains_key(header::WWW_AUTHENTICATE));
    let (status, _, body) = env.call(bearer(get("/jmap/session"), &token)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let session: serde_json::Value = json(&body);
    assert_eq!((session["username"].as_str(), session["apiUrl"].as_str()), (Some("ada@example.test"), Some("/jmap/")));
    let logins = {
        let fake = fake.lock().unwrap();
        (fake.app_logins, fake.password_logins)
    };
    assert_eq!(logins, (1, 0), "the proxy signs in with the app password");
    assert!(session["capabilities"].get("urn:ietf:params:jmap:websocket").is_none());
    let (status, _, body) = env.call(bearer(send("POST", "/jmap/", r#"{"using":[],"methodCalls":[]}"#), &token)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("user:ada@example.test"));

    // CalendarCraft: the hook names the calendar home, `/caldav/…` goes on as the mailbox.
    let (_, _, body) = env.call(as_user(get("/api/calendar/calendarcraft.js"), &ada_cookie)).await;
    let hook = String::from_utf8_lossy(&body).to_string();
    assert!(hook.contains("window.calendarcraftHost") && hook.contains("\"/caldav/ada%40example.test/\"") && hook.contains("/api/calendar/remote"), "{hook}");
    let (_, _, body) = env.call(as_user(get("/api/calendar/calendarcraft.js"), &bob_cookie)).await;
    assert!(String::from_utf8_lossy(&body).contains("\"accounts\":[]"), "no mailbox, no calendar account");
    let dav = |method: &str, uri: &str, cookie: &str| {
        let mut req = as_user(send(method, uri, "<d:propfind xmlns:d=\"DAV:\"/>"), cookie);
        req.headers_mut().insert("x-requested-with", "artcraft".parse().unwrap());
        req.headers_mut().insert("depth", "1".parse().unwrap());
        req
    };
    let bare = env.app.clone().oneshot(as_user(send("PROPFIND", "/caldav/ada%40example.test/", ""), &ada_cookie)).await.unwrap();
    assert_eq!(bare.status(), StatusCode::FORBIDDEN, "only the app's own requests");
    let (status, _, _) = env.call(dav("PROPFIND", "/caldav/bob%40elsewhere.org/", &bob_cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no mailbox");
    let (status, _, _) = env.call(dav("PROPFIND", "/caldav/ada%40example.test/", "artcraft_session=none")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let app_logins = fake.lock().unwrap().app_logins;
    let (status, headers, body) = env.call(dav("PROPFIND", "/caldav/ada%40example.test/", &ada_cookie)).await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(status, StatusCode::MULTI_STATUS, "{text}");
    assert!(text.contains("<d:href>/caldav/ada%40example.test/default/</d:href>"), "paths come back on this origin: {text}");
    // The fake answers only under /dav/cal/ (its path in the reply is rewritten like the hrefs).
    assert!(text.contains("PROPFIND /caldav/ada%40example.test/ depth=1 user=ada@example.test"), "{text}");
    assert_eq!(headers.get("etag").and_then(|v| v.to_str().ok()), Some("\"7\""));
    assert_eq!(fake.lock().unwrap().app_logins, app_logins + 1, "with the app password");
    let (status, _, _) = env.call(dav("PROPPATCH", "/caldav/ada%40example.test/", &ada_cookie)).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _, _) = env.call(dav("GET", "/caldav/ada%40example.test/%2e%2e/%2e%2e/jmap/session", &ada_cookie)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Other calendar servers: never this machine or a private network.
    let mut remote = dav("PROPFIND", "/api/calendar/remote", &ada_cookie);
    remote.headers_mut().insert("x-calendar-url", format!("{url}/dav/cal/ada%40example.test/").parse().unwrap());
    let (status, _, _) = env.call(remote).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "127.0.0.1 is refused");
    let mut remote = dav("GET", "/api/calendar/remote", &ada_cookie);
    remote.headers_mut().insert("x-calendar-url", "file:///etc/passwd".parse().unwrap());
    let (status, _, _) = env.call(remote).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Settings › Account: how to set up another mail app.
    let (_, _, body) = env.call(as_user(get("/api/mail/me"), &ada_cookie)).await;
    let me: artcraft_cloud_types::UserMail = json(&body);
    let client = me.client.expect("client settings");
    assert_eq!((client.host.as_str(), client.username.as_str(), client.imap_port, client.smtp_port), ("mail.example.test", "ada@example.test", 993, 465));
    assert_eq!(me.password_sync, "synced");

    // A password change reaches the mailbox; the proxy keeps working (it has its own credential).
    let change = |cookie: &str, current: &str, next: &str| as_user(send("POST", "/api/me/password", &format!(r#"{{"current":"{current}","next":"{next}"}}"#)), cookie);
    let (status, headers, body) = env.call(change(&ada_cookie, "ada-password", "ada-second-pw")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json::<artcraft_cloud_types::PasswordChanged>(&body).mail, "synced");
    let ada_cookie = headers.get(header::SET_COOKIE).and_then(|v| v.to_str().ok()).unwrap().split(';').next().unwrap().to_string();
    assert_eq!(fake.lock().unwrap().account("ada").password, "ada-second-pw");
    assert_eq!(env.call(bearer(get("/jmap/session"), &token)).await.0, StatusCode::OK);
    // The mail server is down: the artcraftCloud password changes, the answer says the mailbox
    // did not follow, and the retry brings it in line.
    fake.lock().unwrap().fail_updates = true;
    let (status, headers, body) = env.call(change(&ada_cookie, "ada-second-pw", "ada-third-pw")).await;
    assert_eq!(status, StatusCode::OK);
    let changed: artcraft_cloud_types::PasswordChanged = json(&body);
    assert!(changed.mail == "failed" && changed.error.is_some(), "{changed:?}");
    let ada_cookie = headers.get(header::SET_COOKIE).and_then(|v| v.to_str().ok()).unwrap().split(';').next().unwrap().to_string();
    let (_, _, body) = env.call(as_user(get("/api/mail/me"), &ada_cookie)).await;
    assert_eq!(json::<artcraft_cloud_types::UserMail>(&body).password_sync, "failed");
    assert_eq!(fake.lock().unwrap().account("ada").password, "ada-second-pw");
    for file in ["db.sqlite", "db.sqlite-wal"] {
        let bytes = std::fs::read(env._dir.path().join(file)).unwrap_or_default();
        assert!(!bytes.windows(12).any(|w| w == b"ada-third-pw"), "the waiting password is sealed ({file})");
    }
    fake.lock().unwrap().fail_updates = false;
    crate::http::mail::retry_pending(&env.state).await;
    assert_eq!(fake.lock().unwrap().account("ada").password, "ada-third-pw");
    let (_, _, body) = env.call(as_user(get("/api/mail/me"), &ada_cookie)).await;
    assert_eq!(json::<artcraft_cloud_types::UserMail>(&body).password_sync, "synced");

    // Only options go to the server, never MailCraft's mail.
    let (status, _, _) = env.call(as_user(send("PUT", "/api/settings/mailcraft", r#"{"mailcraft.ui.v1":"{}","mailcraft.mailbox.v1":"{\"mail\":1}"}"#), &ada_cookie)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, _, body) = env.call(as_user(get("/api/settings/mailcraft"), &ada_cookie)).await;
    assert_eq!(String::from_utf8_lossy(&body), r#"{"mailcraft.ui.v1":"{}"}"#);

    // Who may do what.
    let (status, _, _) = env.call(as_user(get("/api/mail/config"), &ada_cookie)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = env.call(as_user(send("PUT", &format!("/api/users/{}/email", bob.id), r#"{"email":"x@y.org"}"#), &ada_cookie)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A disabled person keeps the mailbox, but it is suspended on the mail server (other mail apps
    // stop too) and the proxy turns them away, until they are enabled again. An admin's own
    // permission tweak in Stalwart survives both.
    fake.lock().unwrap().accounts.iter_mut().find(|a| a.name == "ada").unwrap().permissions = serde_json::json!({"@type": "Merge", "enabledPermissions": {}, "disabledPermissions": {"emailSend": true}});
    let (status, _, body) = env.call(send("POST", &format!("/api/users/{}/disable", ada.id), "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json::<artcraft_cloud_types::PasswordChanged>(&body).mail, "synced");
    assert!(fake.lock().unwrap().account("ada").suspended());
    assert_eq!(env.call(bearer(get("/jmap/session"), &token)).await.0, StatusCode::UNAUTHORIZED);
    let (_, _, body) = env.call(get("/api/users")).await;
    assert!(json::<Vec<artcraft_cloud_types::User>>(&body).iter().any(|u| u.id == ada.id && u.mail_suspended));
    let (status, _, body) = env.call(send("POST", &format!("/api/users/{}/enable", ada.id), "")).await;
    assert_eq!((status, json::<artcraft_cloud_types::PasswordChanged>(&body).mail.as_str()), (StatusCode::OK, "synced"));
    assert_eq!(fake.lock().unwrap().account("ada").permissions, serde_json::json!({"@type": "Merge", "enabledPermissions": {}, "disabledPermissions": {"emailSend": true}}), "only artcraftCloud's suspension is lifted");
    fake.lock().unwrap().accounts.iter_mut().find(|a| a.name == "ada").unwrap().permissions = serde_json::json!({"@type": "Inherit"});
    assert_eq!(env.call(bearer(get("/jmap/session"), &token)).await.0, StatusCode::OK);
    // With the mail server down the person is still disabled, and the suspension catches up.
    fake.lock().unwrap().fail_updates = true;
    let (_, _, body) = env.call(send("POST", &format!("/api/users/{}/disable", ada.id), "")).await;
    assert_eq!(json::<artcraft_cloud_types::PasswordChanged>(&body).mail, "failed");
    let (_, _, body) = env.call(get("/api/users")).await;
    assert!(json::<Vec<artcraft_cloud_types::User>>(&body).iter().any(|u| u.id == ada.id && u.disabled && !u.mail_suspended && u.mail_access_error.is_some()));
    fake.lock().unwrap().fail_updates = false;
    crate::http::mail::reconcile_access(&env.state).await;
    assert!(fake.lock().unwrap().account("ada").suspended());
    env.call(send("POST", &format!("/api/users/{}/enable", ada.id), "")).await;
    assert!(!fake.lock().unwrap().account("ada").suspended());
    assert_eq!(fake.lock().unwrap().account("ada").permissions, serde_json::json!({"@type": "Inherit"}));
    assert_eq!(env.call(bearer(get("/jmap/../api/account"), &token)).await.0, StatusCode::BAD_REQUEST);

    // Unlink keeps the mailbox on the server; deleting needs the address typed again.
    let (status, _, _) = env.call(send("DELETE", &format!("/api/users/{}/mailbox", ada.id), r#"{"delete":true,"confirm":"nope@example.test"}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, body) = env.call(send("DELETE", &format!("/api/users/{}/mailbox", ada.id), r#"{"delete":false}"#)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json::<artcraft_cloud_types::UserMail>(&body).mailbox, None);
    assert_eq!(fake.lock().unwrap().accounts.len(), 1, "unlinked, not deleted");
    assert!(fake.lock().unwrap().account("ada").apps.is_empty(), "artcraftCloud's app password is revoked");
    let (status, _, _) = env.call(bearer(get("/jmap/session"), &token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the token no longer opens anything");
    let (status, _, body) = env.call(send("PUT", &format!("/api/users/{}/email", ada.id), r#"{"email":"ada@example.test","create_mailbox":true}"#)).await;
    assert_eq!(status, StatusCode::CONFLICT, "an existing account is not taken over");
    assert!(String::from_utf8_lossy(&body).contains("already exists"));
    // Made without the password at hand: a generated one until bob signs in.
    let (status, _, body) = env.call(send("PUT", &format!("/api/users/{}/email", bob.id), r#"{"email":"bob@example.test","create_mailbox":true}"#)).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(json::<artcraft_cloud_types::UserMail>(&body).password_sync, "pending");
    assert_eq!(fake.lock().unwrap().accounts.len(), 2);
    assert_ne!(fake.lock().unwrap().account("bob").password, "bob-password");
    env.login("bob", "bob-password").await;
    for _ in 0..100 {
        if fake.lock().unwrap().account("bob").password == "bob-password" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(fake.lock().unwrap().account("bob").password, "bob-password", "set at sign-in");
    // An admin reset reaches the mailbox too.
    let (status, _, body) = env.call(send("POST", &format!("/api/users/{}/password", bob.id), r#"{"password":"bob-reset-pw"}"#)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json::<artcraft_cloud_types::PasswordChanged>(&body).mail, "synced");
    assert_eq!(fake.lock().unwrap().account("bob").password, "bob-reset-pw");
    let (status, _, _) = env.call(send("DELETE", &format!("/api/users/{}/mailbox", bob.id), r#"{"delete":true,"confirm":"BOB@example.test"}"#)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fake.lock().unwrap().accounts.len(), 1, "deleted on the server");
}

#[tokio::test]
async fn users_change_their_own_password() {
    let env = Env::boot().await;
    env.call(Request::builder().method("POST").uri("/api/users").body(Body::from(r#"{"username":"ada","password":"ada-password"}"#)).unwrap()).await;
    let first = env.login("ada", "ada-password").await;
    let second = env.login("ada", "ada-password").await;
    let change = |cookie: &str, body: &str| {
        let mut req = Request::builder().method("POST").uri("/api/me/password").body(Body::from(body.to_string())).unwrap();
        req.headers_mut().insert(header::COOKIE, cookie.parse().unwrap());
        req
    };
    let (status, _, _) = env.call(change(&first, r#"{"current":"wrong-password","next":"ada-new-password"}"#)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = env.call(change(&first, r#"{"current":"ada-password","next":"short"}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = env.call(change(&first, r#"{"current":"ada-password","next":"ada-password"}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, headers, body) = env.call(change(&first, r#"{"current":"ada-password","next":"ada-new-password"}"#)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json::<artcraft_cloud_types::PasswordChanged>(&body).mail, "none");
    let fresh = headers.get(header::SET_COOKIE).and_then(|v| v.to_str().ok()).unwrap().split(';').next().unwrap().to_string();
    let me = |cookie: &str| {
        let mut req = Request::builder().uri("/api/me").body(Body::empty()).unwrap();
        req.headers_mut().insert(header::COOKIE, cookie.parse().unwrap());
        req
    };
    assert_eq!(env.call(me(&fresh)).await.0, StatusCode::OK, "this browser stays signed in");
    assert_eq!(env.call(me(&second)).await.0, StatusCode::UNAUTHORIZED, "other sessions end");
    assert_eq!(env.call(me(&first)).await.0, StatusCode::UNAUTHORIZED);
    env.login("ada", "ada-new-password").await;
}
