//! People: accounts and groups (admins only). Only the super admin promotes and demotes admins;
//! the server enforces it, the page only hides what would be refused.

use crate::i18n::{t, tf};
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
    let email = RwSignal::new(String::new());
    let mailbox = RwSignal::new(true);
    let served = crate::mail::served_domains();
    let served_here = move || {
        crate::mail::domain_of(&email.get()).is_some_and(|d| served.with(|list| list.contains(&d)))
    };
    let group_name = RwSignal::new(String::new());
    let tick = RwSignal::new(0u32);
    // Every write after an await uses try_set: the page may be gone by the time a reply comes back
    // (switching pages while a list loads used to abort the whole shell here).
    Effect::new(move |_| {
        tick.get();
        ui.tick.get();
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
            <header class="toolbar"><nav class="crumbs"><span class="crumb">{t("People")}</span></nav></header>
            <section class="card">
                <h2>{t("Add a person")}</h2>
                <form class="form-row" on:submit=move |ev| {
                    ev.prevent_default();
                    let body = serde_json::json!({
                        "username": username.get_untracked(),
                        "password": password.get_untracked(),
                        "display_name": Some(display.get_untracked()).filter(|d| !d.trim().is_empty()),
                        "role": role.get_untracked(),
                        "email": Some(email.get_untracked().trim().to_string()).filter(|e| !e.is_empty()),
                        "create_mailbox": mailbox.get_untracked() && served_here(),
                    });
                    let wanted_mailbox = mailbox.get_untracked() && served_here();
                    wasm_bindgen_futures::spawn_local(async move {
                        match api::post_json::<_, User>("/api/users", &body).await {
                            Ok(user) => {
                                if wanted_mailbox && !user.mailbox {
                                    ui.say(t("The account was created, but the mail server did not create the mailbox. Try again from Email."));
                                } else if user.mailbox {
                                    ui.say(tf("{name} was added with the mailbox {address}.", &[("name", &user.display_name), ("address", &user.email.clone().unwrap_or_default())]));
                                }
                                let _ = email.try_set(String::new());
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
                    <input class="field" placeholder=t("Username") autocomplete="off" prop:value=move || username.get() on:input=move |ev| username.set(event_target_value(&ev)) />
                    <input class="field" placeholder=t("Full name (optional)") autocomplete="off" prop:value=move || display.get() on:input=move |ev| display.set(event_target_value(&ev)) />
                    <input class="field" type="email" placeholder=t("Email (optional)") autocomplete="off" prop:value=move || email.get() on:input=move |ev| email.set(event_target_value(&ev)) />
                    <input class="field" placeholder=t("Password (10+ characters)") type="password" autocomplete="new-password" prop:value=move || password.get() on:input=move |ev| password.set(event_target_value(&ev)) />
                    <Show when=is_super>
                        <select class="field narrow" aria-label=t("Role") prop:value=move || role.get() on:change=move |ev| role.set(event_target_value(&ev))>
                            <option value="user">{t("User")}</option>
                            <option value="admin">{t("Admin")}</option>
                        </select>
                    </Show>
                    <button type="submit" class="button primary" disabled=move || username.get().trim().is_empty() || password.get().len() < 10>{icon(Icon::Plus)}<span>{t("Add")}</span></button>
                </form>
                <Show when=served_here>
                    <label class="check mailbox-check">
                        <input type="checkbox" prop:checked=move || mailbox.get() on:change=move |ev| mailbox.set(event_target_checked(&ev)) />
                        <span>{move || tf("Create the mailbox {address} on the mail server", &[("address", &email.get().trim().to_ascii_lowercase())])}</span>
                    </label>
                </Show>
            </section>
            <section class="card">
                <h2>{t("Accounts")}</h2>
                <table class="file-table">
                    <thead><tr><th class="name-col">{t("Name")}</th><th>{t("Role")}</th><th>{t("Status")}</th><th class="actions-col"><span class="sr-only">{t("Actions")}</span></th></tr></thead>
                    <tbody>
                        <For each=move || users.get() key=|u| (u.id, u.role.clone(), u.disabled, u.email.clone(), u.mailbox, u.mail_sync.clone()) let(user)>
                            <tr class="row">
                                <td class="name-col"><span class="file-name static">
                                    <span class="avatar">{initials(&user.display_name)}</span>
                                    <span class="who"><strong>{user.display_name.clone()}</strong><span class="muted">{user.username.clone()}
                                        {user.email.clone().map(|email| view! { " · "<span class="user-email" class:has-mailbox=user.mailbox title=if user.mailbox { t("Has a mailbox on the mail server") } else { t("Email address only") }>{user.mailbox.then(|| icon(Icon::Mail))}{email}</span> })}
                                        {match user.mail_sync.as_str() {
                                            "pending" => Some(view! { <span class="mail-sync" title=t("The mailbox still has a generated password. It becomes their artcraftCloud password at their next sign-in or password change; until then other mail apps cannot sign in.")>{t("mail password at next sign-in")}</span> }.into_any()),
                                            "failed" => Some(view! { <span class="mail-sync bad" title=t("Their last password change did not reach the mail server. artcraftCloud retries every few minutes and at their next sign-in.")>{t("mail password not updated")}</span> }.into_any()),
                                            _ => None,
                                        }}
                                    </span></span>
                                </span></td>
                                <td>{if user.super_admin { view! { <span class="badge primary">{icon(Icon::Shield)}{t("Super admin")}</span> }.into_any() } else if user.role == "admin" { view! { <span class="badge">{t("Admin")}</span> }.into_any() } else { view! { <span class="muted">{t("User")}</span> }.into_any() }}</td>
                                <td>{if user.disabled { view! { <span class="badge danger">{t("Disabled")}</span> }.into_any() } else { view! { <span class="muted">{t("Active")}</span> }.into_any() }}</td>
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
                                                    {if admin { t("Make user") } else { t("Make admin") }}
                                                </button>
                                            })}
                                            {(can_touch || mine).then(|| {
                                                let name = name.clone();
                                                view! { <button type="button" class="button" on:click=move |_| ui.open_modal(Modal::ResetPassword { id, username: name.clone() })>{t("Reset password")}</button> }
                                            })}
                                            {(can_touch || mine).then(|| {
                                                let name = name.clone();
                                                view! { <button type="button" class="button" on:click=move |_| ui.open_modal(Modal::UserEmail { id, username: name.clone() })>{t("Email")}</button> }
                                            })}
                                            {(can_touch && !mine).then(|| view! {
                                                <button type="button" class="button" class:danger=!disabled on:click=move |_| set_enabled(id, disabled, reload)>
                                                    {if disabled { t("Enable") } else { t("Disable") }}
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
                    <h2>{t("Groups")}</h2>
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
                        <input class="field" placeholder=t("New group name") prop:value=move || group_name.get() on:input=move |ev| group_name.set(event_target_value(&ev)) />
                        <button type="submit" class="button" disabled=move || group_name.get().trim().is_empty()>{icon(Icon::Plus)}<span>{t("Create group")}</span></button>
                    </form>
                </div>
                <Show when=move || groups.get().is_empty()><p class="empty">{t("No groups yet. Groups let you share with a team in one step.")}</p></Show>
                <ul class="groups">
                    <For each=move || groups.get() key=|g| (g.id, g.members.join(",")) let(group)>
                        <li class="group">
                            <div class="group-head">
                                <span class="avatar group">{icon(Icon::People)}</span>
                                <strong>{group.name.clone()}</strong>
                                <span class="muted">{format!("{} {}", group.members.len(), if group.members.len() == 1 { "member" } else { "members" })}</span>
                                <button type="button" class="icon-button danger" title=t("Delete group") aria-label=t("Delete group") on:click={
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
                                            <button type="button" aria-label=t("Remove from group") on:click=move |_| {
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
                                <select class="bare" aria-label=t("Add a member") on:change={
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
