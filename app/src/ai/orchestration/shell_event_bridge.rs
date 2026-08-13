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
//! | ModelEvent::AfterBlockCompleted         | CommandFinished { exit }  |
//!
//! ## Exit code propagation
//!
//! `AfterBlockCompletedEvent` now carries `exit_code: i32` (added in this
//! commit). The bridge reads it directly — no optimistic assumption.
//!
//! ## Architecture
//!
//! The bridge is a GPUI model that subscribes to `ModelEventDispatcher`.
//! It holds a `SessionDispatchMap` (session→dispatch_id) and an active
//! session tracker. On each event, it translates to `DcsHookEvent` and
//! calls `transition_worker` on the orchestration store.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;


use ::ai::agent::orchestration::executor::DcsHookEvent;
use ::ai::agent::orchestration::OrchestrationStore;

use parking_lot::Mutex;
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
    /// Track the currently active session for events that don't carry session_id.
    active_session_id: parking_lot::Mutex<Option<SessionId>>,
}

impl ShellEventBridge {
    pub fn new(dispatch_map: Arc<SessionDispatchMap>) -> Self {
        Self {
            dispatch_map,
            active_session_id: parking_lot::Mutex::new(None),
        }
    }

    /// Update the active session ID. Called when a Precmd event (which carries
    /// session_id) is observed.
    pub fn set_active_session(&self, session_id: SessionId) {
        *self.active_session_id.lock() = Some(session_id);
    }

    /// Resolve the dispatch_id for the active session.
    fn dispatch_for_active(&self) -> Option<String> {
        let guard = self.active_session_id.lock();
        guard.as_ref().and_then(|sid| self.dispatch_map.get(sid))
    }

    /// Process a `ModelEvent` and return the corresponding `(dispatch_id, DcsHookEvent)`
    /// if one was generated. Returns `None` for irrelevant events or when no
    /// dispatch is registered for the active session.
    pub fn translate_event(&self, event: &ModelEvent) -> Option<(String, DcsHookEvent)> {
        match event {
            // Bootstrapped carries session_id — direct lookup.
            ModelEvent::Handler(AnsiHandlerEvent::Bootstrapped { session_id, .. }) => {
                let dispatch_id = self.dispatch_map.get(session_id)?;
                Some((dispatch_id, DcsHookEvent::Bootstrapped { shell_path: None }))
            }

            // Precmd carries session_id via the Handler event, but AnsiHandlerEvent::Precmd
            // itself doesn't include it. We track it via set_active_session() called
            // externally (or via ModelEventDispatcher subscription).
            ModelEvent::Handler(AnsiHandlerEvent::Precmd) => {
                let dispatch_id = self.dispatch_for_active()?;
                Some((dispatch_id, DcsHookEvent::Precmd))
            }

            // Preexec = command started.
            ModelEvent::Handler(AnsiHandlerEvent::Preexec) => {
                let dispatch_id = self.dispatch_for_active()?;
                Some((dispatch_id, DcsHookEvent::PromptStarted))
            }

            // AfterBlockCompleted carries the real exit_code.
            ModelEvent::AfterBlockCompleted(ev) => {
                let dispatch_id = self.dispatch_for_active()?;
                Some((
                    dispatch_id,
                    DcsHookEvent::CommandFinished {
                        exit_code: ev.exit_code,
                    },
                ))
            }

            _ => None,
        }
    }
}

/// GPUI Entity implementation — ShellEventBridge is a passive observer
/// model. It emits no events of its own.
impl warpui::Entity for ShellEventBridge {
    type Event = ();
}

/// Subscribe the bridge to a `ModelEventDispatcher`. On each terminal model
/// event, translate it to a `DcsHookEvent` and apply the worker state
/// transition via the orchestration store.
///
/// Call this once during app initialization (per window or globally).
pub fn subscribe_bridge(
    bridge: &warpui::ModelHandle<ShellEventBridge>,
    model_events: &warpui::ModelHandle<ModelEventDispatcher>,
    ctx: &mut warpui::AppContext,
) {
    bridge.update(ctx, |_, ctx| {
        ctx.subscribe_to_model(model_events, |bridge, event, ctx| {
            // Track active session from events that carry session_id.
            if let ModelEvent::Handler(AnsiHandlerEvent::Bootstrapped { session_id, .. }) = event {
                bridge.set_active_session(*session_id);
            }

            // Translate and apply.
            if let Some((dispatch_id, dcs_event)) = bridge.translate_event(event) {
                let target = dcs_event.target_state();
                if let Some(next_state) = target {
                    let store = ::ai::agent::orchestration::connection::store();
                    if let Err(e) = store.transition_worker(&dispatch_id, next_state) {
                        log::warn!(
                            "orchestration: worker transition failed for {}: {e}",
                            dispatch_id
                        );
                    }
                }
            }
            // No need to notify — the bridge is a passive observer.
            let _ = ctx;
        });
    });
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
