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
}

impl Env {
    async fn boot() -> Self {
        let dir = tempfile::tempdir().expect("temp");
        let config = Config::for_tests(dir.path());
        let boot = bootstrap(&config).expect("bootstrap");
        let app = router(boot.state);
        let mut env = Self { _dir: dir, app, cookie: String::new() };
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
        if !headers.contains_key(header::CONTENT_TYPE) && matches!(method.as_str(), "POST" | "PUT" | "PATCH") && !path.contains("/content") && !path.ends_with("/upload") && path != "/dav" && !path.starts_with("/dav/") {
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
    assert_eq!(status, StatusCode::NO_CONTENT);
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
    assert_eq!(status, StatusCode::NO_CONTENT);

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
