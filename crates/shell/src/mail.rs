//! Mail: the Stalwart server the super admin connects (Mail page), each person's email address and
//! mailbox (People › Email), and the signed-in person's own account (Settings › Account).
//!
//! The server keeps the admin credential and every mailbox password sealed; none of them reaches
//! this page. MailCraft gets a short-lived token instead (`/api/mail/mailcraft.js`).

use artcraft_cloud_types::{
    MailClient, MailConfig, MailDomain, MailTest, PasswordChanged, User, UserMail,
};
use leptos::prelude::*;

use crate::api;
use crate::i18n::{t, tf};
use crate::icons::{icon, Icon};
use crate::modal::Dialog;
use crate::route::Route;
use crate::ui::ui;

/// The part after `@`, lower case, when `email` looks like an address.
pub fn domain_of(email: &str) -> Option<String> {
    let (local, domain) = email.trim().rsplit_once('@')?;
    (!local.is_empty() && domain.contains('.')).then(|| domain.to_ascii_lowercase())
}

/// Domain names the mail server serves (admins see names only).
pub fn served_domains() -> RwSignal<Vec<String>> {
    let served = RwSignal::new(Vec::<String>::new());
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(list) = api::get_json::<Vec<MailDomain>>("/api/mail/domains").await {
            let _ = served.try_set(list.into_iter().map(|d| d.name).collect());
        }
    });
    served
}

#[component]
pub fn MailPage() -> impl IntoView {
    let ui = ui();
    let config = RwSignal::new(MailConfig::default());
    let base_url = RwSignal::new(String::new());
    let auth = RwSignal::new(String::from("bearer"));
    let create_key = RwSignal::new(true);
    let public_host = RwSignal::new(String::new());
    let username = RwSignal::new(String::new());
    let secret = RwSignal::new(String::new());
    let test = RwSignal::new(None::<MailTest>);
    let testing = RwSignal::new(false);
    let domains = RwSignal::new(Vec::<MailDomain>::new());
    let new_domain = RwSignal::new(String::new());
    let tick = RwSignal::new(0u32);
    let load_domains = move || {
        wasm_bindgen_futures::spawn_local(async move {
            match api::get_json::<Vec<MailDomain>>("/api/mail/domains").await {
                Ok(list) => {
                    let _ = domains.try_set(list);
                }
                Err(err) => {
                    let _ = domains.try_set(Vec::new());
                    if config.with_untracked(|c| !c.base_url.is_empty()) {
                        ui.say(err);
                    }
                }
            }
        });
    };
    Effect::new(move |_| {
        tick.get();
        wasm_bindgen_futures::spawn_local(async move {
            match api::get_json::<MailConfig>("/api/mail/config").await {
                Ok(c) => {
                    let _ = base_url.try_set(c.base_url.clone());
                    let _ = public_host.try_set(c.public_host.clone());
                    let _ = auth.try_set(if c.auth.is_empty() {
                        "bearer".into()
                    } else {
                        c.auth.clone()
                    });
                    let _ = username.try_set(c.username.clone());
                    let configured = !c.base_url.is_empty();
                    let _ = config.try_set(c);
                    if configured {
                        load_domains();
                    }
                }
                Err(err) => ui.say(err),
            }
        });
    });
    let run_test = move || {
        let _ = testing.try_set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let result =
                api::json::<MailTest>("POST", "/api/mail/test", &serde_json::json!({})).await;
            let _ = testing.try_set(false);
            match result {
                Ok(result) => {
                    let _ = test.try_set(Some(result));
                }
                Err(err) => {
                    let _ = test.try_set(Some(MailTest {
                        ok: false,
                        message: err,
                        ..MailTest::default()
                    }));
                }
            }
        });
    };
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = serde_json::json!({
            "base_url": base_url.get_untracked().trim(),
            "auth": auth.get_untracked(),
            "username": username.get_untracked().trim(),
            "secret": Some(secret.get_untracked()).filter(|s| !s.is_empty()),
            "create_api_key": auth.get_untracked() == "basic" && create_key.get_untracked(),
            "public_host": public_host.get_untracked().trim(),
        });
        let made_key = auth.get_untracked() == "basic" && create_key.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            match api::json::<MailConfig>("PUT", "/api/mail/config", &body).await {
                Ok(saved) => {
                    let _ = secret.try_set(String::new());
                    if made_key && saved.auth == "bearer" {
                        ui.say(t(
                            "API key created and stored. The admin password was not kept.",
                        ));
                    } else {
                        ui.say(t("Mail server saved."));
                    }
                    let _ = tick.try_update(|n| *n += 1);
                    run_test();
                }
                Err(err) => ui.say(err),
            }
        });
    };
    let disconnect = move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            match api::delete("/api/mail/config").await {
                Ok(()) => {
                    let _ = test.try_set(None);
                    let _ = domains.try_set(Vec::new());
                    ui.say(t("Disconnected. Mailboxes stay on the mail server; people keep their email addresses."));
                    let _ = tick.try_update(|n| *n += 1);
                }
                Err(err) => ui.say(err),
            }
        });
    };
    let add_domain = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let name = new_domain.get_untracked();
        wasm_bindgen_futures::spawn_local(async move {
            match api::post_json::<_, MailDomain>(
                "/api/mail/domains",
                &serde_json::json!({"name": name.trim()}),
            )
            .await
            {
                Ok(domain) => {
                    let _ = new_domain.try_set(String::new());
                    ui.say(tf(
                        "{domain} added. Publish its DNS records below.",
                        &[("domain", &domain.name)],
                    ));
                    load_domains();
                }
                Err(err) => ui.say(err),
            }
        });
    };
    let configured = move || config.with(|c| !c.base_url.is_empty());
    // A stored secret counts only for the sign-in kind it was saved with.
    let stored = move || config.with(|c| c.has_secret && c.auth == auth.get());
    let suggested = move || config.with(|c| c.suggested_url.clone());
    view! {
        <section class="drive settings-page mail-page">
            <header class="toolbar"><nav class="crumbs"><span class="crumb">{t("Mail")}</span></nav></header>
            <p class="muted intro">{t("Connect a Stalwart mail server. People whose email address is on one of its domains can get a mailbox there. MailCraft opens it without asking for a password, and other mail apps sign in with the address and the artcraftCloud password.")}</p>

            <form class="card settings-card" on:submit=save>
                <h2>{t("Mail server")}</h2>
                <div class="setting">
                    <div class="setting-text"><strong>{t("Server address")}</strong><span class="muted">{t("Stalwart's web address as this server reaches it, e.g. http://stalwart:8080 with the compose mail profile.")}</span></div>
                    <input class="field" type="url" required placeholder=move || { let s = suggested(); if s.is_empty() { "https://mail.example.com".to_string() } else { s } }
                        prop:value=move || base_url.get() on:input=move |ev| base_url.set(event_target_value(&ev)) />
                </div>
                <Show when=move || !suggested().is_empty() && base_url.get() != suggested()>
                    <div class="reaches">
                        <span class="muted">{move || tf("This server was started with ARTCRAFT_MAIL_URL={url}.", &[("url", &suggested())])}</span>
                        <button type="button" class="button small" on:click=move |_| base_url.set(suggested())>{t("Use it")}</button>
                    </div>
                </Show>
                <div class="setting">
                    <div class="setting-text"><strong>{t("Sign in with")}</strong><span class="muted">{t("An API key from Stalwart (Account › Credentials › API Keys) is recommended. An admin login also works, and can make the key for you.")}</span></div>
                    <select class="field narrow" aria-label=t("Sign in with") on:change=move |ev| auth.set(event_target_value(&ev))>
                        <option value="bearer" prop:selected=move || auth.get() == "bearer">{t("API key (recommended)")}</option>
                        <option value="basic" prop:selected=move || auth.get() == "basic">{t("Admin account (fallback)")}</option>
                    </select>
                </div>
                <Show when=move || auth.get() == "basic">
                    <div class="setting">
                        <div class="setting-text"><strong>{t("Admin account")}</strong><span class="muted">{t("The account name, e.g. admin or admin@example.com.")}</span></div>
                        <input class="field" autocomplete="off" required prop:value=move || username.get() on:input=move |ev| username.set(event_target_value(&ev)) />
                    </div>
                    <label class="check key-check">
                        <input type="checkbox" prop:checked=move || create_key.get() on:change=move |ev| create_key.set(event_target_checked(&ev)) />
                        <span>{t("Use this login once to create an API key, and store only the key (recommended)")}</span>
                    </label>
                    <Show when=move || !create_key.get()>
                        <p class="muted fallback-note">{t("The admin password is then kept on this server (encrypted). An API key can be revoked on its own without changing the admin password.")}</p>
                    </Show>
                </Show>
                <div class="setting">
                    <div class="setting-text">
                        <strong>{move || if auth.get() == "basic" { t("Password") } else { t("API key") }}</strong>
                        <span class="muted">{move || if auth.get() == "basic" && create_key.get() { t("Used once to create the API key, then forgotten.") } else if stored() { t("Stored encrypted and never shown. Leave empty to keep it.") } else { t("Stored encrypted on this server and never shown again.") }}</span>
                    </div>
                    <input class="field" type="password" autocomplete="new-password"
                        placeholder=move || if stored() { "••••••••••" } else { "" }
                        prop:value=move || secret.get() on:input=move |ev| secret.set(event_target_value(&ev)) />
                </div>
                <div class="setting">
                    <div class="setting-text"><strong>{t("Mail host for other apps")}</strong><span class="muted">{t("What people enter as IMAP and SMTP server in other mail apps. Leave empty to use the MX host of their domain.")}</span></div>
                    <input class="field" autocomplete="off" placeholder="mail.example.com" prop:value=move || public_host.get() on:input=move |ev| public_host.set(event_target_value(&ev)) />
                </div>
                <div class="form-actions">
                    <Show when=configured>
                        <button type="button" class="button danger" on:click=disconnect>{t("Disconnect")}</button>
                        <button type="button" class="button" disabled=move || testing.get() on:click=move |_| run_test()>{move || if testing.get() { t("Testing…") } else { t("Test connection") }}</button>
                    </Show>
                    <button type="submit" class="button primary" disabled=move || base_url.get().trim().is_empty() || (!stored() && secret.get().is_empty())>{t("Save")}</button>
                </div>
                {move || test.get().map(|result| view! {
                    <p class="test-result" class:ok=result.ok class:bad=!result.ok role="status">
                        <strong>{if result.ok { t("Connected.") } else { t("Not connected.") }}</strong>" "{result.message.clone()}
                    </p>
                })}
            </form>

            <section class="card settings-card">
                <div class="files-head">
                    <h2>{t("Domains")}</h2>
                    <form class="form-row" on:submit=add_domain>
                        <input class="field" placeholder="example.com" disabled=move || !configured() prop:value=move || new_domain.get() on:input=move |ev| new_domain.set(event_target_value(&ev)) />
                        <button type="submit" class="button" disabled=move || !configured() || !new_domain.get().contains('.')>{icon(Icon::Plus)}<span>{t("Add domain")}</span></button>
                    </form>
                </div>
                <Show when=move || domains.get().is_empty()>
                    <p class="empty">{move || if configured() { t("The mail server has no domains yet. Add the one your addresses end in.") } else { t("Connect a mail server first.") }}</p>
                </Show>
                <For each=move || domains.get() key=|d| (d.id.clone(), d.records.len()) let(domain)>
                    <details class="domain">
                        <summary><strong>{domain.name.clone()}</strong><span class="muted">{tf("{count} DNS records", &[("count", &domain.records.len().to_string())])}</span></summary>
                        <p class="muted">{t("Publish these at your DNS provider so mail reaches this server and is trusted by others.")}</p>
                        {dns_table(domain.records.iter().filter(|r| !r.purpose.is_empty()).cloned().collect())}
                        {
                            let rest: Vec<_> = domain.records.iter().filter(|r| r.purpose.is_empty()).cloned().collect();
                            (!rest.is_empty()).then(|| view! {
                                <details class="dns-more">
                                    <summary class="muted">{tf("{count} optional records (autoconfig, SRV, MTA-STS…)", &[("count", &rest.len().to_string())])}</summary>
                                    {dns_table(rest)}
                                </details>
                            })
                        }
                    </details>
                </For>
            </section>

            <section class="card settings-card">
                <h2>{t("Mailboxes")}</h2>
                <p class="muted">{t("Give someone an email address in People. When its domain is on this server, artcraftCloud creates the mailbox with the person's artcraftCloud password, so they can use other mail apps too, and changes it whenever they change theirs. MailCraft gets in with a separate app password that this server keeps sealed. Disabling a person suspends the mailbox, so no mail app can sign in or send until they are enabled again, while mail sent to them still arrives; removing a mailbox unlinks it unless you choose to delete it.")}</p>
                <button type="button" class="button" on:click=move |_| ui.go(Route::People)>{icon(Icon::People)}<span>{t("Open People")}</span></button>
            </section>
        </section>
    }
}

fn dns_table(records: Vec<artcraft_cloud_types::DnsRecord>) -> impl IntoView {
    view! {
        <table class="dns-table">
            <thead><tr><th>{t("Purpose")}</th><th>{t("Name")}</th><th>{t("Type")}</th><th>{t("Value")}</th></tr></thead>
            <tbody>
                {records.into_iter().map(|record| view! {
                    <tr>
                        <td>{if record.purpose.is_empty() { "—".to_string() } else { record.purpose }}</td>
                        <td><code>{record.name}</code></td>
                        <td>{record.kind}</td>
                        <td><code class="dns-value">{record.value}</code></td>
                    </tr>
                }).collect_view()}
            </tbody>
        </table>
    }
}

/// People › Email: an address, and the mailbox on the mail server.
#[component]
pub fn UserEmailDialog(id: i64, username: String) -> impl IntoView {
    let ui = ui();
    let state = RwSignal::new(None::<UserMail>);
    let email = RwSignal::new(String::new());
    let create = RwSignal::new(true);
    let deleting = RwSignal::new(false);
    let confirm = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let served = served_domains();
    let load = move || {
        wasm_bindgen_futures::spawn_local(async move {
            match api::get_json::<UserMail>(&format!("/api/users/{id}/mail")).await {
                Ok(mail) => {
                    let _ = email.try_set(mail.email.clone().unwrap_or_default());
                    let _ = state.try_set(Some(mail));
                }
                Err(err) => ui.say(err),
            }
        });
    };
    load();
    let linked = move || state.with(|s| s.as_ref().and_then(|s| s.mailbox.clone()));
    let configured = move || state.with(|s| s.as_ref().is_some_and(|s| s.configured));
    let domain_served =
        move || domain_of(&email.get()).is_some_and(|d| served.with(|list| list.contains(&d)));
    let finish = move |result: Result<UserMail, String>, done: String| {
        let _ = busy.try_set(false);
        match result {
            Ok(mail) => {
                ui.say(done);
                let _ = email.try_set(mail.email.clone().unwrap_or_default());
                let _ = state.try_set(Some(mail));
                let _ = deleting.try_set(false);
                let _ = confirm.try_set(String::new());
                ui.refresh();
            }
            Err(err) => ui.say(err),
        }
    };
    let save = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let address = email.get_untracked().trim().to_string();
        let make = create.get_untracked() && domain_served() && linked().is_none();
        let _ = busy.try_set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let body = serde_json::json!({"email": Some(address.clone()).filter(|a| !a.is_empty()), "create_mailbox": make});
            let result =
                api::json::<UserMail>("PUT", &format!("/api/users/{id}/email"), &body).await;
            let done = if make {
                tf("Mailbox {address} created.", &[("address", &address)])
            } else {
                t("Email saved.").to_string()
            };
            finish(result, done);
        });
    };
    let remove = move |delete: bool| {
        let typed = confirm.get_untracked();
        let _ = busy.try_set(true);
        wasm_bindgen_futures::spawn_local(async move {
            let body = serde_json::json!({"delete": delete, "confirm": typed});
            let result =
                api::json::<UserMail>("DELETE", &format!("/api/users/{id}/mailbox"), &body).await;
            let done = if delete {
                t("Mailbox deleted from the mail server.")
            } else {
                t("Mailbox unlinked. It is still on the mail server.")
            };
            finish(result, done.to_string());
        });
    };
    view! {
        <Dialog title=tf("Email for {username}", &[("username", &username)]) wide=true>
            <form class="dialog-body" on:submit=save>
                <label class="stack">
                    <span>{t("Email address")}</span>
                    <input class="field" type="email" autocomplete="off" placeholder="name@example.com" disabled=move || linked().is_some()
                        prop:value=move || email.get() on:input=move |ev| email.set(event_target_value(&ev)) />
                </label>
                <Show when=move || linked().is_none()>
                    {move || if !configured() {
                        view! { <p class="muted">{t("No mail server is connected, so only the address is kept.")}</p> }.into_any()
                    } else if domain_served() {
                        view! {
                            <label class="check">
                                <input type="checkbox" prop:checked=move || create.get() on:change=move |ev| create.set(event_target_checked(&ev)) />
                                <span>{t("Create a mailbox for this address on the mail server")}</span>
                            </label>
                        }.into_any()
                    } else if domain_of(&email.get()).is_some() {
                        view! { <p class="muted">{t("The mail server does not serve this domain, so only the address is kept.")}</p> }.into_any()
                    } else {
                        ().into_any()
                    }}
                </Show>
                <Show when=move || linked().is_some()>
                    <div class="mailbox-box">
                        <p><span class="badge primary">{icon(Icon::Mail)}{t("Mailbox")}</span>" "<strong>{move || linked().unwrap_or_default()}</strong></p>
                        <p class="muted">{t("MailCraft opens this mailbox for them. To change the address, unlink the mailbox first.")}</p>
                        <Show when=move || !deleting.get()>
                            <div class="form-actions">
                                <button type="button" class="button" disabled=move || busy.get() on:click=move |_| remove(false)>{t("Unlink mailbox")}</button>
                                <button type="button" class="button danger" on:click=move |_| deleting.set(true)>{t("Delete mailbox…")}</button>
                            </div>
                        </Show>
                        <Show when=move || deleting.get()>
                            <div class="danger-zone">
                                <p>{move || tf("This deletes {address} and all of its mail on the mail server. It cannot be undone. Type the address to confirm.", &[("address", &linked().unwrap_or_default())])}</p>
                                <input class="field" autocomplete="off" aria-label=t("Address to confirm") prop:value=move || confirm.get() on:input=move |ev| confirm.set(event_target_value(&ev)) />
                                <div class="form-actions">
                                    <button type="button" class="button" on:click=move |_| { deleting.set(false); confirm.set(String::new()); }>{t("Cancel")}</button>
                                    <button type="button" class="button danger" disabled=move || busy.get() || !confirm.get().trim().eq_ignore_ascii_case(&linked().unwrap_or_default())
                                        on:click=move |_| remove(true)>{t("Delete mailbox and mail")}</button>
                                </div>
                            </div>
                        </Show>
                    </div>
                </Show>
                <footer>
                    <button type="button" class="button" on:click=move |_| ui.close_modal()>{t("Close")}</button>
                    <Show when=move || linked().is_none()>
                        <button type="submit" class="button primary" disabled=move || busy.get() || state.get().is_none()>
                            {move || if create.get() && domain_served() && configured() { t("Save and create mailbox") } else { t("Save") }}
                        </button>
                    </Show>
                </footer>
            </form>
        </Dialog>
    }
}

/// Settings › Account: who is signed in, their address, and a new password.
#[component]
pub fn AccountCard() -> impl IntoView {
    let ui = ui();
    let current = RwSignal::new(String::new());
    let next = RwSignal::new(String::new());
    let again = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let mail = RwSignal::new(None::<UserMail>);
    let load = move || {
        wasm_bindgen_futures::spawn_local(async move {
            if let Ok(me) = api::get_json::<UserMail>("/api/mail/me").await {
                let _ = mail.try_set(Some(me));
            }
        })
    };
    load();
    let user = move || {
        ui.user
            .get()
            .map(|u: User| (u.display_name, u.username))
            .unwrap_or_default()
    };
    let mismatch = move || !again.get().is_empty() && again.get() != next.get();
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if next.get_untracked() != again.get_untracked() {
            return;
        }
        let body =
            serde_json::json!({"current": current.get_untracked(), "next": next.get_untracked()});
        let _ = busy.try_set(true);
        let _ = error.try_set(String::new());
        wasm_bindgen_futures::spawn_local(async move {
            let result = api::json::<PasswordChanged>("POST", "/api/me/password", &body).await;
            let _ = busy.try_set(false);
            match result {
                Ok(done) => {
                    let _ = current.try_set(String::new());
                    let _ = next.try_set(String::new());
                    let _ = again.try_set(String::new());
                    match done.mail.as_str() {
                        "synced" => ui.say(t("Password changed for artcraftCloud and your mailbox. You were signed out everywhere else.")),
                        "failed" => ui.say(t("Password changed for artcraftCloud, but not yet for your mailbox. See Other mail apps below.")),
                        _ => ui.say(t("Password changed. You were signed out everywhere else.")),
                    }
                    load();
                }
                Err(err) => {
                    let message = if err.contains("current password") {
                        t("The current password is not right.").to_string()
                    } else if err.contains("too many") {
                        t("Too many tries. Wait a few minutes and try again.").to_string()
                    } else if err.contains("same as the current") {
                        t("The new password is the same as the current one.").to_string()
                    } else {
                        err
                    };
                    let _ = error.try_set(message);
                }
            }
        });
    };
    view! {
        <div class="card settings-card account-card">
            <h2>{t("Account")}</h2>
            <div class="setting">
                <div class="setting-text"><strong>{move || user().0}</strong><span class="muted">{move || user().1}</span></div>
                {move || mail.get().and_then(|m| m.email.clone().map(|email| (email, m.mailbox.is_some()))).map(|(email, mailbox)| view! {
                    <span class="account-mail">{icon(Icon::Mail)}<span>{email}</span>
                        {mailbox.then(|| view! { <button type="button" class="button small" on:click=move |_| ui.go(Route::New { app: "mailcraft".into(), folder: String::new() })>{t("Open MailCraft")}</button> })}
                    </span>
                })}
            </div>
            <form class="password-form" on:submit=submit>
                <div class="setting">
                    <div class="setting-text"><strong>{t("Change password")}</strong><span class="muted">{t("At least 10 characters. Your other sessions are signed out.")}</span></div>
                </div>
                <input type="text" class="sr-only" autocomplete="username" prop:value=move || user().1 readonly tabindex="-1" aria-hidden="true" />
                <div class="password-fields">
                    <label class="stack"><span>{t("Current password")}</span>
                        <input class="field" type="password" autocomplete="current-password" required prop:value=move || current.get() on:input=move |ev| { current.set(event_target_value(&ev)); error.set(String::new()); } /></label>
                    <label class="stack"><span>{t("New password")}</span>
                        <input class="field" type="password" autocomplete="new-password" minlength="10" required prop:value=move || next.get() on:input=move |ev| { next.set(event_target_value(&ev)); error.set(String::new()); } /></label>
                    <label class="stack"><span>{t("Confirm new password")}</span>
                        <input class="field" type="password" autocomplete="new-password" required class:invalid=mismatch prop:value=move || again.get() on:input=move |ev| { again.set(event_target_value(&ev)); error.set(String::new()); } /></label>
                </div>
                <Show when=mismatch><p class="form-error" role="alert">{t("The new passwords do not match.")}</p></Show>
                <Show when=move || !error.get().is_empty()><p class="form-error" role="alert">{move || error.get()}</p></Show>
                <div class="form-actions">
                    <button type="submit" class="button primary" disabled=move || busy.get() || current.get().is_empty() || next.get().chars().count() < 10 || next.get() != again.get()>
                        {icon(Icon::Key)}<span>{t("Change password")}</span>
                    </button>
                </div>
            </form>
            {move || mail.get().and_then(|m| m.client.clone().map(|client| (client, m.password_sync.clone(), m.sync_error.clone()))).map(|(client, sync, sync_error)| view! { <MailClientInfo client sync sync_error /> })}
        </div>
    }
}

/// IMAP/SMTP settings for other mail apps, and whether the mailbox has the password yet.
#[component]
fn MailClientInfo(client: MailClient, sync: String, sync_error: Option<String>) -> impl IntoView {
    let host = if client.host.is_empty() {
        t("Ask your administrator").to_string()
    } else {
        client.host.clone()
    };
    let row = |label: &'static str, value: String| {
        view! { <tr><th scope="row">{label}</th><td><code>{value}</code></td></tr> }
    };
    view! {
        <div class="mail-client">
            <div class="setting-text"><strong>{t("Other mail apps")}</strong><span class="muted">{t("Your mailbox works in any IMAP mail app (Thunderbird, Apple Mail, Outlook, phones) with your email address and your artcraftCloud password.")}</span></div>
            {match sync.as_str() {
                "pending" => Some(view! { <p class="sync-note" role="status">{t("Your mailbox gets your artcraftCloud password the next time you sign in or change it. Until then other mail apps cannot sign in; MailCraft works already.")}</p> }.into_any()),
                "failed" => Some(view! { <p class="sync-note bad" role="alert">{tf("Your last password change has not reached the mail server yet ({error}). Other mail apps still use your previous password; artcraftCloud keeps retrying every few minutes and at your next sign-in.", &[("error", &sync_error.unwrap_or_default())])}</p> }.into_any()),
                _ => None,
            }}
            <table class="client-table">
                <tbody>
                    {row(t("Server"), host)}
                    {row(t("Username"), client.username.clone())}
                    {row(t("Password"), t("Your artcraftCloud password").to_string())}
                    {row(t("Incoming (IMAP)"), tf("port {port}, SSL/TLS", &[("port", &client.imap_port.to_string())]))}
                    {row(t("Outgoing (SMTP)"), tf("port {port}, SSL/TLS (or {starttls} with STARTTLS)", &[("port", &client.smtp_port.to_string()), ("starttls", &client.smtp_starttls_port.to_string())]))}
                </tbody>
            </table>
        </div>
    }
}
