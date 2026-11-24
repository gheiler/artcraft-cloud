//! People: accounts and groups (admins only). Only the super admin promotes and demotes admins;
//! the server enforces it, the page only hides what would be refused.

use artcraft_cloud_types::{Group, User};
use leptos::prelude::*;

use crate::api;
use crate::icons::{icon, Icon};
use crate::modal::initials;
use crate::ui::{ui, Modal};

#[component]
pub fn People() -> impl IntoView {
    let ui = ui();
    let users = RwSignal::new(Vec::<User>::new());
    let groups = RwSignal::new(Vec::<Group>::new());
    let username = RwSignal::new(String::new());
    let display = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let role = RwSignal::new(String::from("user"));
    let group_name = RwSignal::new(String::new());
    let tick = RwSignal::new(0u32);
    // Every write after an await uses try_set: the page may be gone by the time a reply comes back
    // (switching pages while a list loads used to abort the whole shell here).
    Effect::new(move |_| {
        tick.get();
        wasm_bindgen_futures::spawn_local(async move {
            match api::get_json::<Vec<User>>("/api/users").await {
                Ok(list) => {
                    let _ = users.try_set(list);
                }
                Err(err) => ui.say(err),
            }
            match api::get_json::<Vec<Group>>("/api/groups").await {
                Ok(list) => {
                    let _ = groups.try_set(list);
                }
                Err(err) => ui.say(err),
            }
        });
    });
    let reload = move || {
        let _ = tick.try_update(|n| *n += 1);
    };
    let me = move || ui.user.get();
    let is_super = move || me().is_some_and(|u| u.super_admin);
    view! {
        <div class="drive people">
            <header class="toolbar"><nav class="crumbs"><span class="crumb">"People"</span></nav></header>
            <section class="card">
                <h2>"Add a person"</h2>
                <form class="form-row" on:submit=move |ev| {
                    ev.prevent_default();
                    let body = serde_json::json!({
                        "username": username.get_untracked(),
                        "password": password.get_untracked(),
                        "display_name": Some(display.get_untracked()).filter(|d| !d.trim().is_empty()),
                        "role": role.get_untracked(),
                    });
                    wasm_bindgen_futures::spawn_local(async move {
                        match api::post_json::<_, User>("/api/users", &body).await {
                            Ok(_) => {
                                let _ = username.try_set(String::new());
                                let _ = display.try_set(String::new());
                                let _ = password.try_set(String::new());
                                let _ = role.try_set("user".into());
                                reload();
                            }
                            Err(err) => ui.say(err),
                        }
                    });
                }>
                    <input class="field" placeholder="Username" autocomplete="off" prop:value=move || username.get() on:input=move |ev| username.set(event_target_value(&ev)) />
                    <input class="field" placeholder="Full name (optional)" autocomplete="off" prop:value=move || display.get() on:input=move |ev| display.set(event_target_value(&ev)) />
                    <input class="field" placeholder="Password (10+ characters)" type="password" autocomplete="new-password" prop:value=move || password.get() on:input=move |ev| password.set(event_target_value(&ev)) />
                    <Show when=is_super>
                        <select class="field narrow" aria-label="Role" prop:value=move || role.get() on:change=move |ev| role.set(event_target_value(&ev))>
                            <option value="user">"User"</option>
                            <option value="admin">"Admin"</option>
                        </select>
                    </Show>
                    <button type="submit" class="button primary" disabled=move || username.get().trim().is_empty() || password.get().len() < 10>{icon(Icon::Plus)}<span>"Add"</span></button>
                </form>
            </section>
            <section class="card">
                <h2>"Accounts"</h2>
                <table class="file-table">
                    <thead><tr><th class="name-col">"Name"</th><th>"Role"</th><th>"Status"</th><th class="actions-col"><span class="sr-only">"Actions"</span></th></tr></thead>
                    <tbody>
                        <For each=move || users.get() key=|u| (u.id, u.role.clone(), u.disabled) let(user)>
                            <tr class="row">
                                <td class="name-col"><span class="file-name static">
                                    <span class="avatar">{initials(&user.display_name)}</span>
                                    <span class="who"><strong>{user.display_name.clone()}</strong><span class="muted">{user.username.clone()}</span></span>
                                </span></td>
                                <td>{if user.super_admin { view! { <span class="badge primary">{icon(Icon::Shield)}"Super admin"</span> }.into_any() } else if user.role == "admin" { view! { <span class="badge">"Admin"</span> }.into_any() } else { view! { <span class="muted">"User"</span> }.into_any() }}</td>
                                <td>{if user.disabled { view! { <span class="badge danger">"Disabled"</span> }.into_any() } else { view! { <span class="muted">"Active"</span> }.into_any() }}</td>
                                <td class="row-actions">
                                    {
                                        let me = me();
                                        let mine = me.as_ref().is_some_and(|m| m.id == user.id);
                                        let am_super = me.as_ref().is_some_and(|m| m.super_admin);
                                        let can_touch = !user.super_admin && (user.role != "admin" || am_super || mine);
                                        let id = user.id;
                                        let name = user.username.clone();
                                        let admin = user.role == "admin";
                                        let disabled = user.disabled;
                                        view! {
                                            {(am_super && !user.super_admin).then(|| view! {
                                                <button type="button" class="button" on:click=move |_| set_role(id, if admin { "user" } else { "admin" }, reload)>
                                                    {if admin { "Make user" } else { "Make admin" }}
                                                </button>
                                            })}
                                            {(can_touch || mine).then(|| {
                                                let name = name.clone();
                                                view! { <button type="button" class="button" on:click=move |_| ui.open_modal(Modal::ResetPassword { id, username: name.clone() })>"Reset password"</button> }
                                            })}
                                            {(can_touch && !mine).then(|| view! {
                                                <button type="button" class="button" class:danger=!disabled on:click=move |_| set_enabled(id, disabled, reload)>
                                                    {if disabled { "Enable" } else { "Disable" }}
                                                </button>
                                            })}
                                        }
                                    }
                                </td>
                            </tr>
                        </For>
                    </tbody>
                </table>
            </section>
            <section class="card">
                <div class="files-head">
                    <h2>"Groups"</h2>
                    <form class="form-row" on:submit=move |ev| {
                        ev.prevent_default();
                        let name = group_name.get_untracked();
                        wasm_bindgen_futures::spawn_local(async move {
                            match api::post_json::<_, Group>("/api/groups", &serde_json::json!({"name": name})).await {
                                Ok(_) => {
                                    let _ = group_name.try_set(String::new());
                                    reload();
                                }
                                Err(err) => ui.say(err),
                            }
                        });
                    }>
                        <input class="field" placeholder="New group name" prop:value=move || group_name.get() on:input=move |ev| group_name.set(event_target_value(&ev)) />
                        <button type="submit" class="button" disabled=move || group_name.get().trim().is_empty()>{icon(Icon::Plus)}<span>"Create group"</span></button>
                    </form>
                </div>
                <Show when=move || groups.get().is_empty()><p class="empty">"No groups yet. Groups let you share with a team in one step."</p></Show>
                <ul class="groups">
                    <For each=move || groups.get() key=|g| (g.id, g.members.join(",")) let(group)>
                        <li class="group">
                            <div class="group-head">
                                <span class="avatar group">{icon(Icon::People)}</span>
                                <strong>{group.name.clone()}</strong>
                                <span class="muted">{format!("{} {}", group.members.len(), if group.members.len() == 1 { "member" } else { "members" })}</span>
                                <button type="button" class="icon-button danger" title="Delete group" aria-label="Delete group" on:click={
                                    let id = group.id;
                                    move |_| {
                                        wasm_bindgen_futures::spawn_local(async move {
                                            if let Err(err) = api::delete(&format!("/api/groups/{id}")).await {
                                                ui.say(err);
                                            }
                                            reload();
                                        });
                                    }
                                }>{icon(Icon::Trash)}</button>
                            </div>
                            <div class="token-field">
                                {group.members.iter().map(|member| {
                                    let id = group.id;
                                    let member = member.clone();
                                    let remove = member.clone();
                                    view! {
                                        <span class="token">{member}
                                            <button type="button" aria-label="Remove from group" on:click=move |_| {
                                                let remove = remove.clone();
                                                wasm_bindgen_futures::spawn_local(async move {
                                                    if let Err(err) = api::delete(&format!("/api/groups/{id}/members/{}", api::enc(&remove))).await {
                                                        ui.say(err);
                                                    }
                                                    reload();
                                                });
                                            }>{icon(Icon::Close)}</button>
                                        </span>
                                    }
                                }).collect_view()}
                                <select class="bare" aria-label="Add a member" on:change={
                                    let id = group.id;
                                    move |ev| {
                                        let username = event_target_value(&ev);
                                        if username.is_empty() {
                                            return;
                                        }
                                        wasm_bindgen_futures::spawn_local(async move {
                                            if let Err(err) = api::call("POST", &format!("/api/groups/{id}/members"), Some(&serde_json::json!({"username": username}))).await {
                                                ui.say(err);
                                            }
                                            reload();
                                        });
                                    }
                                }>
                                    <option value="">"+ Add member"</option>
                                    {
                                        let members = group.members.clone();
                                        move || users.get().into_iter().filter(|u| !members.contains(&u.username) && !u.disabled).map(|u| view! {
                                            <option value=u.username.clone()>{format!("{} ({})", u.display_name, u.username)}</option>
                                        }).collect_view()
                                    }
                                </select>
                            </div>
                        </li>
                    </For>
                </ul>
            </section>
        </div>
    }
}

fn set_enabled(id: i64, enable: bool, reload: impl Fn() + 'static) {
    let ui = ui();
    wasm_bindgen_futures::spawn_local(async move {
        let path = if enable {
            format!("/api/users/{id}/enable")
        } else {
            format!("/api/users/{id}/disable")
        };
        if let Err(err) = api::post_empty(&path).await {
            ui.say(err);
        }
        reload();
    });
}

fn set_role(id: i64, role: &'static str, reload: impl Fn() + 'static) {
    let ui = ui();
    wasm_bindgen_futures::spawn_local(async move {
        match api::post_json::<_, User>(
            &format!("/api/users/{id}/role"),
            &serde_json::json!({"role": role}),
        )
        .await
        {
            Ok(user) => ui.say(format!(
                "{} is now {}.",
                user.display_name,
                if user.role == "admin" {
                    "an admin"
                } else {
                    "a user"
                }
            )),
            Err(err) => ui.say(err),
        }
        reload();
    });
}
