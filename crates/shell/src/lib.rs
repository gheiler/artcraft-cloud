mod api;
mod drive;
mod editor;

use artcraft_cloud_types::{AppStatus, User};
use drive::{Drive, Page};
use editor::{install, Editor, EditorFrame};
use leptos::prelude::*;

#[component]
pub fn App() -> impl IntoView {
    let user = RwSignal::new(None::<User>);
    let page = RwSignal::new(Page::Files(String::new()));
    let toast = RwSignal::new(String::new());
    let apps = RwSignal::new(Vec::<AppStatus>::new());
    let tick = RwSignal::new(0u32);
    let editor = Editor::new();
    install(editor);
    let editor_for_refresh = editor;
    let refresh = Callback::new(move |_: ()| tick.update(|n| *n += 1));

    wasm_bindgen_futures::spawn_local(async move {
        match api::me().await {
            Ok(me) => {
                editor.author.set(me.display_name.clone());
                user.set(Some(me));
                if let Some(window) = web_sys::window() {
                    if let Ok(path) = window.location().pathname() {
                        if let Some(token) = path.strip_prefix("/s/") {
                            page.set(Page::Public(token.trim_matches('/').to_string()));
                        }
                    }
                }
            }
            Err(_) => {
                if let Some(window) = web_sys::window() {
                    if let Ok(path) = window.location().pathname() {
                        if let Some(token) = path.strip_prefix("/s/") {
                            page.set(Page::Public(token.trim_matches('/').to_string()));
                            user.set(None);
                        }
                    }
                }
            }
        }
    });

    view! {
        <div class="shell">
            <Show when=move || user.get().is_none() && !matches!(page.get(), Page::Public(_)) fallback=move || view! {
                <Workspace user=user page=page toast=toast apps=apps tick=tick editor=editor_for_refresh refresh=refresh />
            }>
                <Login user=user editor=editor />
            </Show>
            <Show when=move || !toast.get().is_empty()>
                <button type="button" class="toast" on:click=move |_| toast.set(String::new())>{move || toast.get()}</button>
            </Show>
        </div>
    }
}

#[component]
fn Login(user: RwSignal<Option<User>>, editor: Editor) -> impl IntoView {
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(String::new());
    view! {
        <main class="login">
            <p class="mark">"artcraft"</p>
            <h1>"Cloud"</h1>
            <p class="lede">"Your files, in the Crafting Apps, on one origin."</p>
            <form on:submit=move |ev| {
                ev.prevent_default();
                let username = username.get_untracked();
                let password = password.get_untracked();
                wasm_bindgen_futures::spawn_local(async move {
                    match api::login(&username, &password).await {
                        Ok(me) => {
                            editor.author.set(me.display_name.clone());
                            error.set(String::new());
                            user.set(Some(me));
                        }
                        Err(err) => error.set(err),
                    }
                });
            }>
                <label>"Username"<input prop:value=move || username.get() on:input=move |ev| username.set(event_target_value(&ev)) autocomplete="username" /></label>
                <label>"Password"<input type="password" prop:value=move || password.get() on:input=move |ev| password.set(event_target_value(&ev)) autocomplete="current-password" /></label>
                <button type="submit">"Sign in"</button>
                <p class="error">{move || error.get()}</p>
            </form>
        </main>
    }
}

#[component]
fn Workspace(
    user: RwSignal<Option<User>>,
    page: RwSignal<Page>,
    toast: RwSignal<String>,
    apps: RwSignal<Vec<AppStatus>>,
    tick: RwSignal<u32>,
    editor: Editor,
    refresh: Callback<()>,
) -> impl IntoView {
    view! {
        <aside>
            <div class="brand"><span>"artcraft"</span>"Cloud"</div>
            <nav>
                <button type="button" class="nav" on:click=move |_| page.set(Page::Files(String::new()))>"My files"</button>
                <button type="button" class="nav" on:click=move |_| page.set(Page::Shared)>"Shared"</button>
                <button type="button" class="nav" on:click=move |_| page.set(Page::Trash)>"Trash"</button>
                <Show when=move || user.get().is_some_and(|user| user.role == "admin")>
                    <button type="button" class="nav" on:click=move |_| page.set(Page::Users)>"People"</button>
                </Show>
            </nav>
            <div class="who">
                <span>{move || user.get().map(|user| user.display_name).unwrap_or_default()}</span>
                <button type="button" on:click=move |_| {
                    wasm_bindgen_futures::spawn_local(async move {
                        let _ = api::logout().await;
                        user.set(None);
                    });
                }>"Sign out"</button>
            </div>
        </aside>
        <main>
            <Show when=move || matches!(page.get(), Page::Users) fallback=move || view! {
                <Drive page=page editor=editor toast=toast apps=apps refresh_tick=tick />
            }>
                <People toast=toast />
            </Show>
            <EditorFrame editor=editor refresh=refresh />
        </main>
    }
}

#[component]
fn People(toast: RwSignal<String>) -> impl IntoView {
    let users = RwSignal::new(Vec::<User>::new());
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let tick = RwSignal::new(0u32);
    Effect::new(move |_| {
        tick.get();
        wasm_bindgen_futures::spawn_local(async move {
            match api::get_json::<Vec<User>>("/api/users").await {
                Ok(list) => users.set(list),
                Err(err) => toast.set(err),
            }
        });
    });
    view! {
        <section class="people">
            <h1>"People"</h1>
            <form class="inline" on:submit=move |ev| {
                ev.prevent_default();
                let name = username.get_untracked();
                let pass = password.get_untracked();
                wasm_bindgen_futures::spawn_local(async move {
                    let body = serde_json::json!({"username": name, "password": pass, "role": "user"});
                    match api::post_json::<_, User>("/api/users", &body).await {
                        Ok(_) => {
                            username.set(String::new());
                            password.set(String::new());
                            tick.update(|n| *n += 1);
                        }
                        Err(err) => toast.set(err),
                    }
                });
            }>
                <input placeholder="username" prop:value=move || username.get() on:input=move |ev| username.set(event_target_value(&ev)) />
                <input placeholder="password" type="password" prop:value=move || password.get() on:input=move |ev| password.set(event_target_value(&ev)) />
                <button type="submit">"Create user"</button>
            </form>
            <ul>
                <For each=move || users.get() key=|user| user.id let(user)>
                    <li>
                        <span>{user.username.clone()}" · "{user.role.clone()}</span>
                        <Show when={let disabled = user.disabled; move || !disabled}>
                            <button type="button" on:click=move |_| set_enabled(user.id, false, toast, tick)>"Disable"</button>
                        </Show>
                        <Show when={let disabled = user.disabled; move || disabled}>
                            <button type="button" on:click=move |_| set_enabled(user.id, true, toast, tick)>"Enable"</button>
                        </Show>
                    </li>
                </For>
            </ul>
        </section>
    }
}

fn set_enabled(id: i64, enabled: bool, toast: RwSignal<String>, tick: RwSignal<u32>) {
    wasm_bindgen_futures::spawn_local(async move {
        let path = if enabled { format!("/api/users/{id}/enable") } else { format!("/api/users/{id}/disable") };
        match api::post_empty(&path).await {
            Ok(()) => tick.update(|n| *n += 1),
            Err(err) => toast.set(err),
        }
    });
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
