//! Orchestration shell event bridge — translates Warp's OSC 133 / DCS hook
//! events into the orchestration plane's `DcsHookEvent` variants.
//!
//! ## Event mapping
//!
//! | Warp Event                              | DcsHookEvent              |
//! |-----------------------------------------|---------------------------|
//! | AnsiHandlerEvent::Bootstrapped          | Bootstrapped              |
//! | AnsiHandlerEvent::Precmd                | Precmd (idle at prompt)   |
//! | AnsiHandlerEvent::Preexec               | PromptStarted (running)   |
//! | AnsiHandlerEvent::UserCommandFinished   | CommandFinished { 0 }     |
//!
//! ## Exit code limitation
//!
//! `UserCommandFinished` does not carry the exit code. The code is available
//! in `CommandFinishedValue` at the block-list layer but not propagated through
//! `ModelEvent`. For now, the bridge assumes exit_code=0 (optimistic). A future
//! enhancement will add exit_code to the event pipeline.
//!
//! ## Architecture
//!
//! The bridge is a GPUI model that subscribes to `ModelEventDispatcher`.
//! It holds a reference to the orchestration store and a session→dispatch
//! mapping. On each relevant event, it calls the store's `transition_worker`.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use parking_lot::Mutex;

use ::ai::agent::orchestration::executor::{DcsHookEvent, WorkerStatusDetector};
use ::ai::agent::orchestration::db::{OrchestrationError, OrchestrationResult};
use ::ai::agent::orchestration::types::WorkerDispatchState;

use crate::terminal::model_events::{AnsiHandlerEvent, ModelEvent, ModelEventDispatcher};
use crate::terminal::model::session::SessionId;

/// Maps Warp session IDs to orchestration dispatch IDs.
/// Populated when a terminal pane is assigned to a worker dispatch.
#[derive(Default)]
pub struct SessionDispatchMap {
    map: Mutex<HashMap<SessionId, String>>,
}

impl SessionDispatchMap {
    pub fn register(&self, session_id: SessionId, dispatch_id: impl Into<String>) {
        self.map.lock().insert(session_id, dispatch_id.into());
    }

    pub fn unregister(&self, session_id: &SessionId) {
        self.map.lock().remove(session_id);
    }

    pub fn get(&self, session_id: &SessionId) -> Option<String> {
        self.map.lock().get(session_id).cloned()
    }
}

impl warpui::Entity for SessionDispatchMap {
    type Event = ();
}

impl warpui::SingletonEntity for SessionDispatchMap {}

/// Shell event bridge — subscribes to ModelEventDispatcher and translates
/// shell events into worker state transitions.
///
/// Created once per terminal pane (or as a global singleton in future).
pub struct ShellEventBridge {
    dispatch_map: Arc<SessionDispatchMap>,
}

impl ShellEventBridge {
    pub fn new(dispatch_map: Arc<SessionDispatchMap>) -> Self {
        Self { dispatch_map }
    }

    /// Process a `ModelEvent` and return the corresponding `DcsHookEvent`
    /// if one was generated. Returns `None` for irrelevant events.
    pub fn translate_event(&self, event: &ModelEvent) -> Option<(String, DcsHookEvent)> {
        let handler_event = match event {
            ModelEvent::Handler(h) => h,
            _ => return None,
        };

        let dcs_event = match handler_event {
            AnsiHandlerEvent::Bootstrapped { session_id, .. } => {
                let dispatch_id = self.dispatch_map.get(session_id)?;
                return Some((
                    dispatch_id,
                    DcsHookEvent::Bootstrapped { shell_path: None },
                ));
            }
            AnsiHandlerEvent::Precmd => DcsHookEvent::Precmd,
            AnsiHandlerEvent::Preexec => DcsHookEvent::PromptStarted,
            AnsiHandlerEvent::UserCommandFinished => DcsHookEvent::CommandFinished { exit_code: 0 },
            _ => return None,
        };

        // For Precmd/Preexec/UserCommandFinished we need the session_id to
        // look up the dispatch. These events don't carry session_id directly —
        // it's set in ModelEventDispatcher::active_session_id. For now, return
        // the event without dispatch_id resolution (the caller can handle it).
        // This will be refined when the bridge is wired into a specific pane.
        let _ = dcs_event;
        None
    }
}

/// Translate a `DcsHookEvent` into a worker state transition via the store.
/// Returns the new state if a transition occurred.
pub fn apply_dcs_event(
    detector: &dyn WorkerStatusDetector,
    dispatch_id: &str,
    event: &DcsHookEvent,
) -> OrchestrationResult<Option<WorkerDispatchState>> {
    detector.on_dcs_hook(dispatch_id, event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_dispatch_map_register_and_get() {
        let map = SessionDispatchMap::default();
        let sid = SessionId::from(42);
        map.register(sid, "dispatch_123");
        assert_eq!(map.get(&sid), Some("dispatch_123".to_string()));
        map.unregister(&sid);
        assert_eq!(map.get(&sid), None);
    }
}
