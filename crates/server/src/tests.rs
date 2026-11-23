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
