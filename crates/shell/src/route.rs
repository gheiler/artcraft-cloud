//! URLs of the shell. Every page and every open document has its own address, so a reload or a
//! link brings the same thing back.
//!
//! | URL | Route |
//! |---|---|
//! | `/`, `/files/<folder>` | My files |
//! | `/shared`, `/shared/<id>/<folder>` | Shared with me / inside a share |
//! | `/trash`, `/people` | Trash, People (admins) |
//! | `/s/<token>` | A public link |
//! | `/open/<path>` (`?app=<id>`) | A file of mine, full screen |
//! | `/shared/<id>/open/<path>` | A file inside a share, full screen |
//! | `/new/<app>?folder=<folder>` | A new document, until its first save |

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Files(String),
    Shared,
    Share(i64, String),
    Trash,
    People,
    Public(String),
    Open {
        share: Option<i64>,
        path: String,
        app: Option<String>,
    },
    New {
        app: String,
        folder: String,
    },
}

impl Route {
    pub fn is_doc(&self) -> bool {
        matches!(self, Route::Open { .. } | Route::New { .. })
    }

    pub fn parse(path: &str, query: &str) -> Route {
        let segments: Vec<String> = path
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(decode)
            .collect();
        let param = |key: &str| -> Option<String> {
            query.trim_start_matches('?').split('&').find_map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (k == key).then(|| decode(v)).filter(|v| !v.is_empty())
            })
        };
        let rest = |from: usize| {
            segments
                .get(from..)
                .map(|s| s.join("/"))
                .unwrap_or_default()
        };
        match segments.first().map(String::as_str) {
            None => Route::Files(String::new()),
            Some("files") => Route::Files(rest(1)),
            Some("trash") => Route::Trash,
            Some("people") => Route::People,
            Some("s") => Route::Public(segments.get(1).cloned().unwrap_or_default()),
            Some("open") if segments.len() > 1 => Route::Open {
                share: None,
                path: rest(1),
                app: param("app"),
            },
            Some("new") if segments.len() > 1 => Route::New {
                app: segments[1].clone(),
                folder: param("folder").unwrap_or_default(),
            },
            Some("shared") => match segments.get(1).and_then(|id| id.parse::<i64>().ok()) {
                None => Route::Shared,
                Some(id) if segments.get(2).map(String::as_str) == Some("open") => Route::Open {
                    share: Some(id),
                    path: rest(3),
                    app: param("app"),
                },
                Some(id) => Route::Share(id, rest(2)),
            },
            Some(_) => Route::Files(String::new()),
        }
    }

    pub fn href(&self) -> String {
        match self {
            Route::Files(folder) if folder.is_empty() => "/".into(),
            Route::Files(folder) => format!("/files/{}", encode_path(folder)),
            Route::Shared => "/shared".into(),
            Route::Share(id, folder) if folder.is_empty() => format!("/shared/{id}"),
            Route::Share(id, folder) => format!("/shared/{id}/{}", encode_path(folder)),
            Route::Trash => "/trash".into(),
            Route::People => "/people".into(),
            Route::Public(token) => format!("/s/{token}"),
            Route::Open { share, path, app } => {
                let base = match share {
                    None => format!("/open/{}", encode_path(path)),
                    Some(id) => format!("/shared/{id}/open/{}", encode_path(path)),
                };
                match app {
                    Some(app) => format!("{base}?app={}", encode(app)),
                    None => base,
                }
            }
            Route::New { app, folder } if folder.is_empty() => format!("/new/{app}"),
            Route::New { app, folder } => format!("/new/{app}?folder={}", encode(folder)),
        }
    }

    /// The drive page a document lives in (where the back arrow goes after a direct link).
    pub fn home_of(&self) -> Route {
        match self {
            Route::Open {
                share: None, path, ..
            } => Route::Files(parent(path)),
            Route::Open {
                share: Some(id),
                path,
                ..
            } => Route::Share(*id, parent(path)),
            Route::New { folder, .. } => Route::Files(folder.clone()),
            other => other.clone(),
        }
    }
}

pub fn parent(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(head, _)| head.to_string())
        .unwrap_or_default()
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn encode_path(path: &str) -> String {
    path.split('/').map(encode).collect::<Vec<_>>().join("/")
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len()
                && bytes[i + 1].is_ascii_hexdigit()
                && bytes[i + 2].is_ascii_hexdigit() =>
            {
                let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_round_trip_through_their_urls() {
        let routes = [
            Route::Files(String::new()),
            Route::Files("Projects/Q4 plans".into()),
            Route::Shared,
            Route::Share(7, String::new()),
            Route::Share(7, "sub dir".into()),
            Route::Trash,
            Route::People,
            Route::Public("abcDEF123".into()),
            Route::Open {
                share: None,
                path: "Hola.docx".into(),
                app: None,
            },
            Route::Open {
                share: None,
                path: "Fotos/año 2026/backup #1.zip".into(),
                app: Some("lightcraft".into()),
            },
            Route::Open {
                share: Some(3),
                path: "team/Budget.xlsx".into(),
                app: None,
            },
            Route::Open {
                share: Some(9),
                path: String::new(),
                app: Some("wordcraft".into()),
            },
            Route::New {
                app: "gridcraft".into(),
                folder: String::new(),
            },
            Route::New {
                app: "wordcraft".into(),
                folder: "Letters & notes".into(),
            },
        ];
        for route in routes {
            let href = route.href();
            let (path, query) = href
                .split_once('?')
                .map(|(p, q)| (p.to_string(), q.to_string()))
                .unwrap_or((href.clone(), String::new()));
            assert_eq!(Route::parse(&path, &query), route, "{href}");
        }
    }

    #[test]
    fn documents_know_their_folder() {
        assert_eq!(
            Route::parse("/open/a/b/c.docx", "").home_of(),
            Route::Files("a/b".into())
        );
        assert_eq!(
            Route::parse("/shared/4/open/x.pdf", "").home_of(),
            Route::Share(4, String::new())
        );
        assert!(Route::parse("/new/wordcraft", "").is_doc());
        assert_eq!(
            Route::parse("/something/else", ""),
            Route::Files(String::new())
        );
    }
}
