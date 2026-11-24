use crate::{dom, model};
use serde_json::Value as V;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
};

pub trait MapHost: Send + Sync {
    fn current_project(&self) -> String {
        let selected = model::current_project();
        if !selected.is_empty() {
            return selected;
        }
        dom::value("project")
    }

    fn sync_working_indicator(&self) {}

    fn attach_map_context(&self, _project: &str, text: &str) -> bool {
        let _ = dom::set_value("graph-context-text", text);
        dom::text(
            "graph-context-status",
            "Copy or paste this graph context into your agent prompt.",
        );
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

    fn explain_via_sessions(&self) -> bool {
        false
    }

    fn selected_model(&self, _prefix: &str) -> V {
        V::Null
    }

    fn refresh_history(&self) {}

    fn on_auth_error(&self) {
        dom::hidden("connection-banner", false);
    }

    fn register_failure_localizer(&self, _localizer: fn(&V)) {}

    fn mark_failed_services(&self, _services: &[String]) {}

    fn open_from_omnigraph(&self, _scope: &str, _project_id: &str, _target_id: &str) {}

    fn focus_context_for_file(&self, _project_id: Option<&str>, _path: &str) {}

    fn poll_system_graph_status(&self, _force: bool) {}

    fn refresh_services_map(&self, _project: &str, _config: &str, _catalog: &V, _status: &V) {}

    fn build_changes_overlay(
        &self,
        _run_doc: &V,
        _plan_form_open: bool,
        _checked_seeds: &[String],
        _depth: u64,
        _contracts_raw: &str,
    ) -> V {
        V::Null
    }

    fn refresh_changes_map(
        &self,
        _project: &str,
        _config: &str,
        _catalog: &V,
        _status: &V,
        _overlay: &V,
    ) {
    }

    fn refresh_graph_preserving_pages(&self) {}
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

pub fn current_project() -> String {
    current().current_project()
}

pub fn sync_working_indicator() {
    current().sync_working_indicator();
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

pub fn explain_via_sessions() -> bool {
    current().explain_via_sessions()
}

pub fn selected_model(prefix: &str) -> V {
    current().selected_model(prefix)
}

pub fn refresh_history() {
    current().refresh_history();
}

pub fn on_auth_error() {
    current().on_auth_error();
}

pub fn register_failure_localizer(localizer: fn(&V)) {
    current().register_failure_localizer(localizer);
}

pub fn mark_failed_services(services: &[String]) {
    current().mark_failed_services(services);
}

pub async fn open_from_omnigraph(scope: &str, project_id: &str, target_id: &str) {
    current().open_from_omnigraph(scope, project_id, target_id);
}

pub async fn focus_context_for_file(project_id: Option<&str>, path: &str) {
    current().focus_context_for_file(project_id, path);
}

pub async fn poll_system_graph_status(force: bool) {
    current().poll_system_graph_status(force);
}

pub fn refresh_services_map(project: &str, config: &str, catalog: &V, status: &V) {
    current().refresh_services_map(project, config, catalog, status);
}

pub fn build_changes_overlay(
    run_doc: &V,
    plan_form_open: bool,
    checked_seeds: &[String],
    depth: u64,
    contracts_raw: &str,
) -> V {
    current().build_changes_overlay(run_doc, plan_form_open, checked_seeds, depth, contracts_raw)
}

pub fn refresh_changes_map(project: &str, config: &str, catalog: &V, status: &V, overlay: &V) {
    current().refresh_changes_map(project, config, catalog, status, overlay);
}

pub async fn refresh_graph_preserving_pages() {
    current().refresh_graph_preserving_pages();
}

pub fn copy_context_value() -> V {
    let text = dom::value("graph-context-text");
    let _ = dom::window().navigator().clipboard().write_text(&text);
    dom::text("graph-context-status", "Copied to clipboard.");
    V::Null
}
