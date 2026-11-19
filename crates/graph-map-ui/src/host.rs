use crate::{dom, model};
use serde_json::Value as V;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
};

pub trait MapHost: Send + Sync {
    fn current_project(&self) -> String {
        dom::value("project")
    }

    fn attach_map_context(&self, _project: &str, text: &str) -> bool {
        let _ = dom::set_value("graph-context-text", text);
        dom::text("graph-context-status", "Copy or paste this graph context into your agent prompt.");
        dom::dialog("graph-context-dialog", true);
        false
    }

    fn open_view(&self, _view: &str) -> Pin<Box<dyn Future<Output = ()>>> {
        Box::pin(async {})
    }

    fn run_event_path(&self, project: &str, session: &str, event: u64) -> String {
        format!(
            "/api/projects/{}/graph/run-event{}",
            dom::enc(project),
            dom::query(&[
                ("session", session.to_owned()),
                ("event", event.to_string()),
            ])
        )
    }

    fn explain_path(&self, project: &str) -> String {
        format!("/api/projects/{}/graph/explain", dom::enc(project))
    }

    fn on_auth_error(&self) {
        dom::hidden("connection-banner", false);
    }
}

pub struct StandaloneHost;

impl MapHost for StandaloneHost {}

static HOST: OnceLock<Arc<dyn MapHost>> = OnceLock::new();

pub fn set_host(host: Arc<dyn MapHost>) {
    let _ = HOST.set(host);
}

pub fn current() -> &'static dyn MapHost {
    HOST.get_or_init(|| Arc::new(StandaloneHost)).as_ref()
}

pub fn attach_map_context(project: &str, text: &str) -> bool {
    let _ = dom::window().navigator().clipboard().write_text(text);
    if model::read(|s| !s.host_attached) {
        dom::text(
            "graph-context-status",
            "Copied to clipboard — ready to paste into Claude Code or Codex.",
        );
    }
    current().attach_map_context(project, text)
}

pub async fn open_view(view: &str) {
    current().open_view(view).await;
}

pub fn run_event_path(project: &str, session: &str, event: u64) -> String {
    current().run_event_path(project, session, event)
}

pub fn explain_path(project: &str) -> String {
    current().explain_path(project)
}

pub fn on_auth_error() {
    current().on_auth_error();
}

pub fn copy_context_value() -> V {
    let text = dom::value("graph-context-text");
    let _ = dom::window().navigator().clipboard().write_text(&text);
    dom::text("graph-context-status", "Copied to clipboard.");
    V::Null
}
