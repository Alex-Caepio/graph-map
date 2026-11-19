pub mod api;
pub mod dom;
pub mod graph;
pub mod graph_explanation;
pub mod graph_map;
pub mod host;
pub mod model;

use serde_json::{Value as V, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    init_dom()
}

pub fn init_dom() -> Result<(), JsValue> {
    apply_theme();

    for kind in [
        "click",
        "contextmenu",
        "change",
        "input",
        "submit",
        "keydown",
        "keyup",
        "pointerover",
        "focusin",
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
            if k == "click" && graph_explanation::click(&target) {
                return;
            }
            if k == "keydown" {
                let Some(key) = event.dyn_ref::<web_sys::KeyboardEvent>() else {
                    return;
                };
                if graph_explanation::key(key) || graph_map::key(key) {
                    event.prevent_default();
                    return;
                }
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

    spawn_local(async {
        bootstrap().await;
    });
    Ok(())
}

async fn dispatch(kind: &str, id: &str, action: &str, payload: V, value: &str, checked: bool) {
    match (kind, id, action) {
        ("change", "project", _) => {
            dom::save("graph-map-project", Some(value));
            graph::load(true).await;
            return;
        }
        ("change", _, _) if matches!(payload.as_str(), None) && dom::get("theme-switch").is_some() => {
            // Check theme radio changes
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
