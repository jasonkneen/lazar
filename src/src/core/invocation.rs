//! Per-invocation state for long-lived, multi-session processes.

use std::{
    cell::RefCell,
    collections::HashSet,
    sync::{LazyLock, Mutex},
};

static CANCELLED_SESSIONS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

thread_local! {
    static SESSION_IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct ScopedSession;

impl Drop for ScopedSession {
    fn drop(&mut self) {
        SESSION_IDS.with(|slot| {
            slot.borrow_mut()
                .pop()
                .expect("scoped session guards must be dropped in stack order");
        });
    }
}

/// Bind a session to the current worker thread. Scopes may nest, and dropping
/// the guard restores the preceding session.
pub(crate) fn scope_session(id: impl Into<String>) -> ScopedSession {
    SESSION_IDS.with(|slot| slot.borrow_mut().push(id.into()));
    ScopedSession
}

/// Return the current worker's session, falling back to the one-shot CLI env
/// contract. ACP never mutates the process environment.
pub(crate) fn session_id() -> Option<String> {
    SESSION_IDS
        .with(|slot| slot.borrow().last().cloned())
        .or_else(|| {
            std::env::var("LAZAR_SESSION_ID")
                .ok()
                .filter(|id| !id.is_empty())
        })
}

pub(crate) fn cancel_session(id: &str) {
    CANCELLED_SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(id.to_string());
}

pub(crate) fn clear_cancel(id: &str) {
    CANCELLED_SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(id);
}

pub(crate) fn is_cancelled() -> bool {
    let Some(id) = session_id() else {
        return false;
    };
    is_session_cancelled(&id)
}

pub(crate) fn is_session_cancelled(id: &str) -> bool {
    CANCELLED_SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_scope_is_nested_and_thread_local() {
        let _env_guard = crate::test_support::env_lock();
        let _snapshot = crate::test_support::EnvSnapshot::capture(&["LAZAR_SESSION_ID"]);
        std::env::remove_var("LAZAR_SESSION_ID");
        let outer = scope_session("outer");
        assert_eq!(session_id().as_deref(), Some("outer"));
        assert_eq!(std::thread::spawn(session_id).join().unwrap(), None);
        {
            let _inner = scope_session("inner");
            assert_eq!(session_id().as_deref(), Some("inner"));
        }
        assert_eq!(session_id().as_deref(), Some("outer"));
        drop(outer);
        assert_eq!(session_id(), None);
    }

    #[test]
    fn cancellation_is_keyed_by_scoped_session() {
        let id = format!("cancel-test-{}", std::process::id());
        clear_cancel(&id);
        let _scope = scope_session(id.clone());
        assert!(!is_cancelled());
        cancel_session(&id);
        assert!(is_cancelled());
        clear_cancel(&id);
        assert!(!is_cancelled());
    }
}
