pub mod api;
pub mod dom;
pub mod git_commit_graph;
pub mod graph;
pub mod graph_explanation;
pub mod graph_map;
pub mod graph_viewport;
pub mod host;
pub mod model;

#[cfg(target_arch = "wasm32")]
use serde_json::{Value as V, json};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local;

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) fn assert_no_inline_hex_colors(markup: &str) {
        let bytes = markup.as_bytes();
        for (idx, &b) in bytes.iter().enumerate() {
            if b == b'#' {
                let prev = idx.checked_sub(1).and_then(|p| bytes.get(p)).copied();
                let next = bytes.get(idx + 1).copied();
                let is_entity = prev == Some(b'&');
                let is_url_ref = idx >= 4 && &bytes[idx - 4..idx] == b"url(";
                if !is_entity && !is_url_ref && next.is_some_and(|c| c.is_ascii_hexdigit()) {
                    panic!(
                        "unexpected inline hex color in markup around byte {idx}: {}",
                        &markup[idx..markup.len().min(idx + 16)]
                    );
                }
            }
        }
    }

    pub(crate) fn payloads(markup: &str, action: &str) -> Vec<serde_json::Value> {
        let marker = format!("data-action=\"{action}\"");
        markup
            .match_indices(&marker)
            .map(|(at, _)| {
                let rest = &markup[at..];
                let tag = &rest[..rest.find('>').expect("closed tag")];
                let start = tag.find("data-payload=\"").expect("payload") + 14;
                let raw = &tag[start..start + tag[start..].find('"').expect("quoted")];
                let text = raw
                    .replace("&quot;", "\"")
                    .replace("&#39;", "'")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&");
                serde_json::from_str(&text).expect("JSON payload")
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TopDialogEscapeAction {
    CloseGraphExplanation,
    DelegateToHandlersThenFallbackClose(&'static str),
    CloseDialog(&'static str),
    DelegateToHandlers,
}

fn escape_action_for_top_dialog(top_dialog: Option<&str>) -> TopDialogEscapeAction {
    match top_dialog {
        Some("graph-explanation") => TopDialogEscapeAction::CloseGraphExplanation,
        Some("graph-map") => {
            TopDialogEscapeAction::DelegateToHandlersThenFallbackClose("graph-map")
        }
        Some("graph-link") => {
            TopDialogEscapeAction::DelegateToHandlersThenFallbackClose("graph-link")
        }
        Some("graph-context-dialog") => {
            TopDialogEscapeAction::CloseDialog("graph-context-dialog")
        }
        _ => TopDialogEscapeAction::DelegateToHandlers,
    }
}

#[cfg(target_arch = "wasm32")]
fn close_graph_explanation_cleanly() {
    graph_explanation::stop();
    dom::dialog("graph-explanation", false);
}

#[cfg(target_arch = "wasm32")]
fn close_top_dialog_action(
    action: TopDialogEscapeAction,
    key: &web_sys::KeyboardEvent,
) -> bool {
    match action {
        TopDialogEscapeAction::CloseGraphExplanation => {
            if !graph_explanation::key(key) {
                close_graph_explanation_cleanly();
            }
            true
        }
        TopDialogEscapeAction::CloseDialog(id) => {
            dom::dialog(id, false);
            true
        }
        TopDialogEscapeAction::DelegateToHandlersThenFallbackClose(_)
        | TopDialogEscapeAction::DelegateToHandlers => false,
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    init_dom()
}

#[cfg(target_arch = "wasm32")]
pub fn init_dom() -> Result<(), JsValue> {
    apply_theme();

    for kind in [
        "click",
        "contextmenu",
        "change",
        "input",
        "submit",
        "keydown",
        "pointerdown",
        "pointermove",
        "pointerup",
        "pointercancel",
        "dblclick",
        "pointerover",
        "focusin",
        "focusout",
    ] {
        let k = kind.to_owned();
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            let Some(target) = event
                .target()
                .and_then(|v| v.dyn_into::<web_sys::Element>().ok())
            else {
                return;
            };
            if k == "contextmenu" {
                if let Some(mouse) = event.dyn_ref::<web_sys::MouseEvent>() {
                    if graph_explanation::context_menu(&target, mouse) {
                        event.prevent_default();
                    }
                }
                return;
            }
            if k == "click" {
                dom::sync_widget_from_target(&target);
                if graph_explanation::click(&target) {
                    return;
                }
                if let Some(mouse) = event.dyn_ref::<web_sys::MouseEvent>() {
                    model::set(
                        "pointer-mods",
                        json!({
                            "alt": mouse.alt_key(),
                            "shift": mouse.shift_key(),
                            "ctrl": mouse.ctrl_key() || mouse.meta_key(),
                        }),
                    );
                }
            }
            if k == "focusin" {
                dom::sync_widget_from_target(&target);
                return;
            }
            if k == "focusout" || k == "pointerover" {
                return;
            }
            if k == "keydown" {
                let Some(key) = event.dyn_ref::<web_sys::KeyboardEvent>() else {
                    return;
                };
                let is_escape = key.key() == "Escape";
                let top_dialog = if is_escape {
                    dom::top_open_dialog_id()
                } else {
                    None
                };
                if is_escape
                    && close_top_dialog_action(
                        escape_action_for_top_dialog(top_dialog.as_deref()),
                        key,
                    )
                {
                    event.prevent_default();
                    return;
                }
                if graph_explanation::key(key) || graph_map::key(key) {
                    event.prevent_default();
                    return;
                }
                if is_escape {
                    if let TopDialogEscapeAction::DelegateToHandlersThenFallbackClose(id) =
                        escape_action_for_top_dialog(top_dialog.as_deref())
                    {
                        dom::dialog(id, false);
                        event.prevent_default();
                        return;
                    }
                    if dom::collapse_active_expanded_workspace() {
                        event.prevent_default();
                        return;
                    }
                }
                if dom::handle_a11y_keydown(key) {
                    event.prevent_default();
                }
                return;
            }
            if [
                "pointerdown",
                "pointermove",
                "pointerup",
                "pointercancel",
                "dblclick",
            ]
            .contains(&k.as_str())
            {
                let _ = graph_map::pointer_event(&k, &event);
                return;
            }
            if k == "submit" {
                event.prevent_default();
            }
            let element = if k == "click" {
                target
                    .closest("[data-action],button,a[data-view],input[type=checkbox]")
                    .ok()
                    .flatten()
                    .unwrap_or(target)
            } else {
                target
            };
            if dom::is_disabled(&element) {
                return;
            }
            let id = element.id();
            let action = element
                .get_attribute("data-action")
                .unwrap_or_else(|| id.clone());
            let payload = element
                .get_attribute("data-payload")
                .and_then(|v| serde_json::from_str::<V>(&v).ok())
                .unwrap_or(V::Null);
            if k == "click" && element.tag_name() == "A" && element.has_attribute("data-action") {
                event.prevent_default();
            }
            let value = dom::value_of(&element);
            let checked = dom::checked_of(&element);
            let kind = k.clone();
            spawn_local(async move {
                dispatch(&kind, &id, &action, payload, &value, checked).await;
            });
        });
        dom::document()
            .add_event_listener_with_callback(kind, callback.as_ref().unchecked_ref())?;
        callback.forget();
    }

    graph_map::attach_wheel_listeners();

    let cancel_cb = Closure::<dyn FnMut(web_sys::Event)>::new(|event: web_sys::Event| {
        let Some(target) = event
            .target()
            .and_then(|v| v.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        if target.id() == "graph-explanation" {
            event.prevent_default();
            close_graph_explanation_cleanly();
        }
    });
    dom::document().add_event_listener_with_callback_and_bool(
        "cancel",
        cancel_cb.as_ref().unchecked_ref(),
        true,
    )?;
    cancel_cb.forget();

    let toggle_cb = Closure::<dyn FnMut(web_sys::Event)>::new(|event: web_sys::Event| {
        if let Some(target) = event
            .target()
            .and_then(|v| v.dyn_into::<web_sys::Element>().ok())
        {
            dom::handle_a11y_toggle(&target);
        }
    });
    dom::document().add_event_listener_with_callback_and_bool(
        "toggle",
        toggle_cb.as_ref().unchecked_ref(),
        true,
    )?;
    toggle_cb.forget();

    let close_cb = Closure::<dyn FnMut(web_sys::Event)>::new(|event: web_sys::Event| {
        if let Some(target) = event
            .target()
            .and_then(|v| v.dyn_into::<web_sys::Element>().ok())
        {
            dom::handle_a11y_dialog_close(&target);
        }
    });
    dom::document().add_event_listener_with_callback_and_bool(
        "close",
        close_cb.as_ref().unchecked_ref(),
        true,
    )?;
    close_cb.forget();

    spawn_local(async {
        bootstrap().await;
    });
    Ok(())
}

#[cfg(target_arch = "wasm32")]
async fn dispatch(kind: &str, id: &str, action: &str, payload: V, value: &str, checked: bool) {
    match (kind, id, action) {
        ("change", "project", _) => {
            dom::save("graph-map-project", Some(value));
            model::set_current_project(value);
            api::reconnect_sse_for_project_scope();
            graph::load(true).await;
            return;
        }
        ("change", _, _) if matches!(payload.as_str(), None) && dom::get("theme-switch").is_some() => {
            if let Ok(radios) = dom::document().query_selector_all("input[name='theme']:checked") {
                for i in 0..radios.length() {
                    if let Some(el) = radios.item(i).and_then(|n| n.dyn_into::<web_sys::Element>().ok())
                    {
                        let chosen = dom::value_of(&el);
                        if matches!(chosen.as_str(), "system" | "light" | "dark") {
                            dom::save("graph-map-theme", Some(&chosen));
                            apply_theme();
                        }
                    }
                }
            }
        }
        ("submit", "add-project-form", _) => {
            add_project().await;
            return;
        }
        ("click", "graph-context-copy", _) => {
            let _ = host::copy_context_value();
            return;
        }
        ("click", "graph-context-close", _) => {
            dom::dialog("graph-context-dialog", false);
            return;
        }
        _ => {}
    }

    if graph::event(kind, id, checked).await {
        return;
    }
    let _ = graph_map::event(kind, id, action, &payload, value, checked).await;
}

#[cfg(target_arch = "wasm32")]
fn apply_theme() {
    let saved = dom::storage("graph-map-theme").unwrap_or_else(|| "system".to_owned());
    let dark = match saved.as_str() {
        "dark" => true,
        "light" => false,
        _ => dom::window()
            .match_media("(prefers-color-scheme: dark)")
            .ok()
            .flatten()
            .map(|m| m.matches())
            .unwrap_or(false),
    };
    if let Some(root) = dom::document().document_element() {
        let _ = root.set_attribute("data-theme", if dark { "dark" } else { "light" });
    }
    if let Ok(inputs) = dom::document().query_selector_all("input[name='theme']") {
        for i in 0..inputs.length() {
            if let Some(input) = inputs
                .item(i)
                .and_then(|n| n.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                input.set_checked(input.value() == saved);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub async fn bootstrap() {
    dom::hidden("connecting", false);
    dom::error("connection-banner-text", "");
    match api::get("/api/bootstrap").await {
        Ok(boot) => {
            dom::hidden("connecting", true);
            dom::hidden("connection-banner", true);
            model::write(|s| {
                s.boot = boot.clone();
                s.projects = model::array(&boot["projects"]);
            });
            let projects = model::read(|s| s.projects.clone());
            let preferred = dom::storage("graph-map-project")
                .filter(|saved| projects.iter().any(|p| model::text(p, "id") == *saved))
                .unwrap_or_else(|| model::text(&boot, "selected"));
            model::set_current_project(&preferred);
            dom::options(
                "project",
                &projects,
                |p| {
                    let name = model::text(p, "name");
                    let path = model::text(p, "path");
                    if name.is_empty() {
                        path
                    } else {
                        format!("{name} ({path})")
                    }
                },
                if preferred.is_empty() {
                    None
                } else {
                    Some(preferred.as_str())
                },
            );
            if !preferred.is_empty() {
                dom::set_value("project", &preferred);
            }
            api::start_sse();
            graph::load(true).await;
            let status = model::data("graph");
            if status["graph"].is_object() {
                let _ = graph_map::event("click", "graph-view", "graph-view", &V::Null, "", false).await;
            }
        }
        Err(error) => {
            dom::hidden("connecting", true);
            dom::error("connection-message", &error.message);
            dom::hidden("connection-banner", false);
        }
    }
}

#[cfg(target_arch = "wasm32")]
async fn add_project() {
    let path = dom::value("add-project-path").trim().to_owned();
    if path.is_empty() {
        dom::error("add-project-error", "Enter a local directory path.");
        return;
    }
    dom::error("add-project-error", "");
    match api::post("/api/projects", json!({"path": path})).await {
        Ok(project) => {
            let id = model::text(&project, "id");
            if !id.is_empty() {
                dom::save("graph-map-project", Some(&id));
                dom::set_value("add-project-path", "");
                bootstrap().await;
            }
        }
        Err(error) => {
            dom::error("add-project-error", &error.message);
        }
    }
}

#[cfg(test)]
mod escape_precedence_tests {
    use super::*;

    #[test]
    fn escape_precedence_covers_explanation_map_link() {
        assert_eq!(
            escape_action_for_top_dialog(Some("graph-explanation")),
            TopDialogEscapeAction::CloseGraphExplanation
        );
        assert_eq!(
            escape_action_for_top_dialog(Some("graph-map")),
            TopDialogEscapeAction::DelegateToHandlersThenFallbackClose("graph-map")
        );
        assert_eq!(
            escape_action_for_top_dialog(Some("graph-link")),
            TopDialogEscapeAction::DelegateToHandlersThenFallbackClose("graph-link")
        );
        assert_eq!(
            escape_action_for_top_dialog(None),
            TopDialogEscapeAction::DelegateToHandlers
        );
    }
}
