use serde_json::{Value as V, json};
use std::{cell::RefCell, collections::BTreeMap};

#[derive(Default)]
pub struct State {
    pub boot: V,
    pub projects: Vec<V>,
    pub aborts: BTreeMap<String, web_sys::AbortController>,
    pub epochs: BTreeMap<String, u64>,
    pub timers: BTreeMap<String, i32>,
    pub data: BTreeMap<String, V>,
    pub auth_failed: bool,
    pub host_attached: bool,
    pub view: String,
    pub sid: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

pub fn read<R>(f: impl FnOnce(&State) -> R) -> R {
    STATE.with(|s| f(&s.borrow()))
}

pub fn write<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

pub fn text(v: &V, key: &str) -> String {
    v.get(key)
        .and_then(V::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub fn number(v: &V) -> Option<f64> {
    v.as_f64()
}

pub fn yes(v: &V) -> bool {
    v.as_bool().unwrap_or(false)
}

pub fn array(v: &V) -> Vec<V> {
    v.as_array().cloned().unwrap_or_default()
}

pub fn data(key: &str) -> V {
    read(|s| s.data.get(key).cloned().unwrap_or(V::Null))
}

pub fn set(key: &str, value: V) {
    write(|s| {
        s.data.insert(key.into(), value);
    });
}

pub fn take(key: &str) -> V {
    write(|s| s.data.remove(key).unwrap_or(V::Null))
}

pub fn next(key: &str) -> u64 {
    let abort = write(|s| {
        let next = s.epochs.get(key).copied().unwrap_or(0) + 1;
        s.epochs.insert(key.into(), next);
        (next, s.aborts.remove(key))
    });
    if let Some(controller) = abort.1 {
        controller.abort();
    }
    abort.0
}

pub fn current(key: &str, epoch: u64) -> bool {
    read(|s| s.epochs.get(key).copied() == Some(epoch))
}

pub fn summary(v: &V) -> String {
    for key in ["title", "path", "name", "id"] {
        let text = text(v, key);
        if !text.is_empty() {
            return text;
        }
    }
    json!(v).to_string()
}
