//! Inline SVG icons (24 × 24, 1.75 px strokes in `currentColor`), drawn for artcraftCloud.
//! Inline so they need no request, follow the text colour and scale with the font.

use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Download,
    Share,
    Rename,
    Move,
    Copy,
    Trash,
    Folder,
    FolderPlus,
    File,
    Upload,
    Back,
    Home,
    Sidebar,
    Close,
    OpenWith,
    Search,
    Link,
    People,
    Shared,
    Grip,
    Restore,
    Plus,
    Logout,
    Settings,
    Shield,
    Chevron,
    Mail,
    Key,
}

fn paths(icon: Icon) -> &'static str {
    match icon {
        Icon::Mail => {
            r#"<rect x="3" y="5" width="18" height="14" rx="2"/><path d="m4 7 8 6 8-6"/>"#
        }
        Icon::Key => {
            r#"<circle cx="8" cy="15" r="4"/><path d="m11 12 9-9"/><path d="m17 6 3 3"/><path d="m15 8 2 2"/>"#
        }
        Icon::Download => r#"<path d="M12 4v11"/><path d="m7 10 5 5 5-5"/><path d="M5 20h14"/>"#,
        Icon::Share => {
            r#"<circle cx="18" cy="5.5" r="2.5"/><circle cx="6" cy="12" r="2.5"/><circle cx="18" cy="18.5" r="2.5"/><path d="m8.3 10.8 7.4-4.1M8.3 13.2l7.4 4.1"/>"#
        }
        Icon::Rename => {
            r#"<path d="M4 20h4L19 9a2.1 2.1 0 0 0-4-4L4 16v4Z"/><path d="m13.5 6.5 4 4"/>"#
        }
        Icon::Move => {
            r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/><path d="M9 13h7"/><path d="m13 10 3 3-3 3"/>"#
        }
        Icon::Copy => {
            r#"<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>"#
        }
        Icon::Trash => {
            r#"<path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12"/><path d="M9 7V4h6v3"/>"#
        }
        Icon::Folder => {
            r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>"#
        }
        Icon::FolderPlus => {
            r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/><path d="M12 10v6M9 13h6"/>"#
        }
        Icon::File => {
            r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8Z"/><path d="M14 3v5h5"/>"#
        }
        Icon::Upload => r#"<path d="M12 16V5"/><path d="m7 10 5-5 5 5"/><path d="M5 20h14"/>"#,
        Icon::Back => r#"<path d="M19 12H5"/><path d="m11 6-6 6 6 6"/>"#,
        Icon::Home => r#"<path d="m3 11 9-7 9 7"/><path d="M5 10v10h5v-6h4v6h5V10"/>"#,
        Icon::Sidebar => r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/>"#,
        Icon::Close => r#"<path d="M6 6l12 12M18 6 6 18"/>"#,
        Icon::OpenWith => {
            r#"<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><path d="M17.5 14v7M14 17.5h7"/>"#
        }
        Icon::Search => r#"<circle cx="11" cy="11" r="6.5"/><path d="m20 20-4.4-4.4"/>"#,
        Icon::Link => {
            r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3A4 4 0 0 0 11 18.7l1-1"/>"#
        }
        Icon::People => {
            r#"<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0"/><path d="M16 4.6a3.5 3.5 0 0 1 0 6.8"/><path d="M18 14.2a6.5 6.5 0 0 1 3.5 5.8"/>"#
        }
        Icon::Shared => {
            r#"<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20a6.5 6.5 0 0 1 13 0"/><path d="M17 9v6M14 12h6"/>"#
        }
        Icon::Grip => {
            r#"<circle cx="9" cy="6" r="1"/><circle cx="15" cy="6" r="1"/><circle cx="9" cy="12" r="1"/><circle cx="15" cy="12" r="1"/><circle cx="9" cy="18" r="1"/><circle cx="15" cy="18" r="1"/>"#
        }
        Icon::Restore => r#"<path d="M4 12a8 8 0 1 0 2.3-5.6"/><path d="M4 4v4h4"/>"#,
        Icon::Plus => r#"<path d="M12 5v14M5 12h14"/>"#,
        Icon::Logout => {
            r#"<path d="M14 4h4a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-4"/><path d="M10 16l-4-4 4-4"/><path d="M6 12h10"/>"#
        }
        Icon::Shield => {
            r#"<path d="M12 3 5 6v5c0 4.5 3 8.3 7 10 4-1.7 7-5.5 7-10V6Z"/><path d="m9 12 2 2 4-4"/>"#
        }
        Icon::Chevron => r#"<path d="m9 6 6 6-6 6"/>"#,
        Icon::Settings => {
            r#"<path d="M4 7h10M18 7h2M4 17h4M12 17h8"/><circle cx="16" cy="7" r="2"/><circle cx="10" cy="17" r="2"/>"#
        }
    }
}

/// An icon that inherits the text colour. Decorative: the button around it carries the label.
pub fn icon(icon: Icon) -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor"
            stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
            inner_html=paths(icon)></svg>
    }
}

/// Path of the app's own icon (from its repository's `assets/app-icon`, MIT OR Apache-2.0).
pub fn app_icon_src(app: &str) -> String {
    format!("/assets/icons/{app}.png")
}

pub fn app_icon(app: &str, class: &'static str) -> impl IntoView {
    view! { <img class=class src=app_icon_src(app) alt="" width="20" height="20" loading="lazy" /> }
}
